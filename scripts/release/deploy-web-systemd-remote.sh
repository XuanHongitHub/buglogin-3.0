#!/usr/bin/env bash
set -Eeuo pipefail

mode=${1:-}
stage=${2:-}
commit=${3:-}
expected_sha=${4:-}
[[ "$(id -u)" -eq 0 ]] || { echo "must run as root" >&2; exit 20; }
[[ "$mode" == activate && "$commit" =~ ^[0-9a-f]{40}$ && "$expected_sha" =~ ^[0-9a-f]{64}$ ]] || { echo "invalid deployment arguments" >&2; exit 21; }
[[ "$stage" =~ ^/tmp/buglogin3-web-${commit}-[0-9a-f]{32}$ && -d "$stage" && ! -L "$stage" ]] || { echo "invalid staging path" >&2; exit 22; }

unit=buglogin3-web.service
root=/opt/buglogin3-web
releases="$root/releases"
current="$root/current"
bundle="$stage/buglogin3-web.tar.gz"
manifest="$stage/release-manifest.json"
mkdir -p /run/lock
exec 9>/run/lock/buglogin3-web-deploy.lock
flock -x 9

for file in "$bundle" "$manifest" "$stage/deploy-web-systemd-remote.sh"; do
  [[ -f "$file" && ! -L "$file" ]] || { echo "missing regular deployment input" >&2; exit 23; }
done
[[ "$(sha256sum -- "$bundle" | cut -d ' ' -f1)" == "$expected_sha" ]] || { echo "bundle checksum mismatch" >&2; exit 24; }
python3 - "$manifest" "$commit" "$expected_sha" <<'PY'
import json, sys
with open(sys.argv[1], encoding="utf-8-sig") as file:
    manifest = json.load(file)
if manifest.get("commit") != sys.argv[2] or manifest.get("target") != "buglogin3-web-linux-x64" or manifest.get("bundleSha256") != sys.argv[3]:
    raise SystemExit("manifest identity or checksum mismatch")
PY

unit_path=$(systemctl show "$unit" -p FragmentPath --value)
exec_path=$(systemctl show "$unit" -p ExecStart --value)
[[ "$unit_path" == /etc/systemd/system/buglogin3-web.service && "$exec_path" == *"/opt/buglogin3-web/current/server.js"* ]] || { echo "systemd unit is outside BugLogin 3 web target" >&2; exit 26; }
[[ "$(systemctl is-active "$unit")" == active ]] || { echo "BugLogin 3 web service is not active" >&2; exit 27; }

install -d -o root -g buglogin3web -m 0750 "$releases"
stamp=$(date -u +%Y%m%dT%H%M%SZ)
release_id="$stamp-${commit:0:12}"
release="$releases/$release_id"
[[ ! -e "$release" && ! -L "$release" ]] || { echo "release path already exists" >&2; exit 28; }
install -d -o buglogin3web -g buglogin3web -m 0750 "$release"

# Reject path traversal and links before extracting the locally built standalone bundle.
python3 - "$bundle" "$release" <<'PY'
import pathlib, sys, tarfile
archive, destination = sys.argv[1:]
root = pathlib.Path(destination).resolve()
with tarfile.open(archive, "r:gz") as tar:
    members = tar.getmembers()
    if not members or len(members) > 100000:
        raise SystemExit("invalid web bundle member count")
    total = 0
    for member in members:
        target = (root / member.name).resolve()
        if target != root and root not in target.parents:
            raise SystemExit("web bundle contains a path traversal")
        if member.issym() or member.islnk() or not (member.isfile() or member.isdir()):
            raise SystemExit("web bundle contains unsupported file types")
        total += member.size
        if total > 2 * 1024 * 1024 * 1024:
            raise SystemExit("web bundle exceeds 2 GiB")
    tar.extractall(root, members=members, filter="data")
PY
chown -R buglogin3web:buglogin3web "$release"
chmod -R u=rwX,g=rX,o= "$release"
[[ -f "$release/server.js" && -d "$release/.next-web" && -d "$release/public" ]] || { rm -rf -- "$release"; echo "bundle is missing standalone server files" >&2; exit 29; }

old_target=""
if [[ -L "$current" ]]; then old_target=$(readlink -f -- "$current"); elif [[ -e "$current" ]]; then rm -rf -- "$release"; echo "current release path is not a symlink" >&2; exit 30; fi
if [[ -n "$old_target" && "$old_target" != "$releases/"* ]]; then rm -rf -- "$release"; echo "current release escapes the BugLogin web root" >&2; exit 30; fi

activate() {
  ln -s -- "$release" "$root/current.next"
  mv -Tf -- "$root/current.next" "$current"
  systemctl restart "$unit"
  for _ in $(seq 1 30); do
    if systemctl is-active --quiet "$unit" && curl --fail --silent --max-time 3 http://127.0.0.1:12342/ | grep -Fq 'Mỗi tài khoản'; then return 0; fi
    sleep 1
  done
  return 1
}

if ! activate; then
  if [[ -n "$old_target" ]]; then
    ln -s -- "$old_target" "$root/current.rollback"
    mv -Tf -- "$root/current.rollback" "$current"
    systemctl restart "$unit"
    for _ in $(seq 1 20); do
      if systemctl is-active --quiet "$unit" && curl --fail --silent --max-time 3 http://127.0.0.1:12342/ >/dev/null; then
        echo "new web release failed; previous release restored" >&2
        exit 31
      fi
      sleep 1
    done
  fi
  echo "new web release failed and rollback health check failed" >&2
  exit 32
fi
printf 'deployment_id=%s\nrelease=%s\ncommit=%s\nbundle_sha256=%s\n' "$release_id" "$release" "$commit" "$expected_sha"
