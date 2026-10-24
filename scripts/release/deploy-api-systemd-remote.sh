#!/usr/bin/env bash
set -Eeuo pipefail

mode=${1:-}
if [[ "$(id -u)" -ne 0 ]]; then
  echo "deployment helper must run as root" >&2
  exit 20
fi

unit=buglogin3-api.service
api_target=/opt/buglogin3/buglogin-api
admin_target=/opt/buglogin3/buglogin-admin-password-reset
rollback_dir=/opt/buglogin3/rollback
active_marker="$rollback_dir/active-deployment.state"
mkdir -p /run/lock
exec 9>/run/lock/buglogin3-api-deploy.lock
flock -x 9

is_sha() { [[ "$1" =~ ^[0-9a-f]{64}$ ]]; }
is_commit() { [[ "$1" =~ ^[0-9a-f]{40}$ ]]; }
is_deployment_id() { [[ "$1" =~ ^[0-9]{8}T[0-9]{6}Z-[0-9a-f]{40}-[0-9a-f]{16}$ ]]; }

read_state() {
  local file=$1 key value
  declare -gA state=()
  while IFS='=' read -r key value; do
    [[ -n "$key" && "$key" != *[!a-z_]* && -z "${state[$key]+present}" ]] || return 1
    state[$key]=$value
  done < "$file"
}

write_marker() {
  local deployment_id=$1 api_sha=$2 admin_sha=$3 tmp="$active_marker.tmp.$$"
  printf 'deployment_id=%s\napi_sha256=%s\nadmin_sha256=%s\n' "$deployment_id" "$api_sha" "$admin_sha" > "$tmp"
  chown root:buglogin3 "$tmp"
  chmod 0640 "$tmp"
  mv -fT -- "$tmp" "$active_marker"
}

restore_previous() {
  local old_api_backup=$1 old_admin_present=$2 old_admin_backup=$3 deployment_id=$4
  local restore_api="$api_target.restore-$deployment_id"
  install -o root -g buglogin3 -m 0750 -- "$old_api_backup" "$restore_api"
  mv -fT -- "$restore_api" "$api_target"
  if [[ "$old_admin_present" == true ]]; then
    local restore_admin="$admin_target.restore-$deployment_id"
    install -o root -g buglogin3 -m 0750 -- "$old_admin_backup" "$restore_admin"
    mv -fT -- "$restore_admin" "$admin_target"
  else
    rm -f -- "$admin_target"
  fi
  systemctl restart "$unit"
  for _ in $(seq 1 20); do
    if systemctl is-active --quiet "$unit" && curl --fail --silent --max-time 2 http://127.0.0.1:12343/v1/readyz | grep -Eq '"status"[[:space:]]*:[[:space:]]*"ready"'; then
      return 0
    fi
    sleep 1
  done
  echo "rollback failed; inspect only $unit and BugLogin API logs" >&2
  return 1
}

if [[ "$mode" == rollback ]]; then
  deployment_id=${2:?missing deployment ID}
  expected_api_sha=${3:?missing deployed API hash}
  expected_admin_sha=${4:?missing deployed admin hash}
  if ! is_deployment_id "$deployment_id" || ! is_sha "$expected_api_sha" || ! is_sha "$expected_admin_sha"; then
    echo "rollback identity or hashes are invalid" >&2
    exit 21
  fi
  state_file="$rollback_dir/deployment-$deployment_id.state"
  if [[ -L "$state_file" || ! -f "$state_file" ]] || ! read_state "$state_file"; then
    echo "deployment rollback receipt is missing or invalid" >&2
    exit 22
  fi
  declare -A receipt=()
  for key in "${!state[@]}"; do receipt[$key]=${state[$key]}; done
  deployment_commit=${deployment_id:16:40}
  if [[ "${receipt[state]:-}" != active || "${receipt[deployment_id]:-}" != "$deployment_id" || "${receipt[release_commit]:-}" != "$deployment_commit" || "${receipt[deployed_api_sha256]:-}" != "$expected_api_sha" || "${receipt[deployed_admin_sha256]:-}" != "$expected_admin_sha" ]]; then
    echo "rollback receipt does not match the requested active deployment" >&2
    exit 23
  fi
  if [[ -L "$active_marker" || ! -f "$active_marker" ]] || ! read_state "$active_marker"; then
    echo "active deployment marker is missing or invalid; refusing rollback" >&2
    exit 24
  fi
  if [[ "${state[deployment_id]:-}" != "$deployment_id" || "${state[api_sha256]:-}" != "$expected_api_sha" || "${state[admin_sha256]:-}" != "$expected_admin_sha" ]]; then
    echo "a newer deployment is active; refusing to overwrite it" >&2
    exit 25
  fi
  if [[ ! -f "$api_target" || -L "$api_target" || "$(sha256sum -- "$api_target" | cut -d ' ' -f 1)" != "$expected_api_sha" ]]; then
    echo "active API binary differs from the requested deployment; refusing rollback" >&2
    exit 26
  fi
  if [[ ! -f "$admin_target" || -L "$admin_target" || "$(sha256sum -- "$admin_target" | cut -d ' ' -f 1)" != "$expected_admin_sha" ]]; then
    echo "active operator binary differs from the requested deployment; refusing rollback" >&2
    exit 27
  fi
  old_api_backup=${receipt[old_api_backup]:-}
  old_admin_present=${receipt[old_admin_present]:-}
  old_admin_backup=${receipt[old_admin_backup]:-}
  if [[ "$old_api_backup" != "$rollback_dir/buglogin-api-$deployment_id-${receipt[old_api_sha256]:-}" || ! -f "$old_api_backup" || -L "$old_api_backup" || "$(sha256sum -- "$old_api_backup" | cut -d ' ' -f 1)" != "${receipt[old_api_sha256]:-}" ]]; then
    echo "recorded predecessor API backup is missing or changed; refusing rollback" >&2
    exit 28
  fi
  if [[ "$old_admin_present" == true ]]; then
    if [[ "$old_admin_backup" != "$rollback_dir/buglogin-admin-password-reset-$deployment_id-${receipt[old_admin_sha256]:-}" || ! -f "$old_admin_backup" || -L "$old_admin_backup" || "$(sha256sum -- "$old_admin_backup" | cut -d ' ' -f 1)" != "${receipt[old_admin_sha256]:-}" ]]; then
      echo "recorded predecessor operator backup is missing or changed; refusing rollback" >&2
      exit 29
    fi
  elif [[ "$old_admin_present" != false || -n "$old_admin_backup" ]]; then
    echo "rollback receipt has invalid predecessor operator state" >&2
    exit 30
  fi
  if [[ "${receipt[old_marker_present]:-false}" == true ]]; then
    if ! is_deployment_id "${receipt[old_marker_deployment_id]:-}" || ! is_sha "${receipt[old_marker_api_sha256]:-}" || ! is_sha "${receipt[old_marker_admin_sha256]:-}"; then
      echo "rollback receipt has an invalid predecessor deployment marker" >&2
      exit 30
    fi
  elif [[ "${receipt[old_marker_present]:-}" != false || -n "${receipt[old_marker_deployment_id]:-}" || -n "${receipt[old_marker_api_sha256]:-}" || -n "${receipt[old_marker_admin_sha256]:-}" ]]; then
    echo "rollback receipt has inconsistent predecessor deployment marker state" >&2
    exit 30
  fi
  if ! restore_previous "$old_api_backup" "$old_admin_present" "$old_admin_backup" "$deployment_id"; then
    exit 31
  fi
  if [[ "${receipt[old_marker_present]:-false}" == true ]]; then
    if ! write_marker "${receipt[old_marker_deployment_id]}" "${receipt[old_marker_api_sha256]}" "${receipt[old_marker_admin_sha256]}"; then
      rm -f -- "$active_marker"
      echo "predecessor was restored, but its active marker could not be restored" >&2
      exit 31
    fi
  else
    rm -f -- "$active_marker"
  fi
  receipt[state]=rolled_back
  state_tmp="$state_file.tmp.$$"
  {
    for key in "${!receipt[@]}"; do printf '%s=%s\n' "$key" "${receipt[$key]}"; done
  } | sort > "$state_tmp"
  chown root:buglogin3 "$state_tmp"
  chmod 0640 "$state_tmp"
  mv -fT -- "$state_tmp" "$state_file"
  printf 'deployment_id=%s\nresult=rolled_back\nrestored_api_sha256=%s\nrolled_back_at_utc=%s\n' \
    "$deployment_id" "${receipt[old_api_sha256]}" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" > "$rollback_dir/rollback-$deployment_id.log"
  chown root:buglogin3 "$rollback_dir/rollback-$deployment_id.log"
  chmod 0640 "$rollback_dir/rollback-$deployment_id.log"
  echo "deployment=rolled_back deployment_id=$deployment_id api_sha256=${receipt[old_api_sha256]}"
  exit 0
fi

if [[ "$mode" != activate ]]; then
  echo "usage: helper activate <stage> <commit> <api-sha> <admin-sha> <openapi-sha> | rollback <deployment-id> <api-sha> <admin-sha>" >&2
  exit 32
fi

stage_dir=${2:?missing staging directory}
release_commit=${3:?missing source commit}
expected_api_sha=${4:?missing API hash}
expected_admin_sha=${5:?missing admin tool hash}
expected_openapi_sha=${6:?missing OpenAPI hash}
if ! is_commit "$release_commit" || [[ ! "$stage_dir" =~ ^/tmp/buglogin3-api-${release_commit}-[0-9a-f]{32}$ ]]; then
  echo "staging path or commit is invalid" >&2
  exit 33
fi
if ! is_sha "$expected_api_sha" || ! is_sha "$expected_admin_sha" || ! is_sha "$expected_openapi_sha"; then
  echo "release hashes are invalid" >&2
  exit 34
fi
if [[ -L "$stage_dir" || ! -d "$stage_dir" ]]; then
  echo "staging directory is invalid" >&2
  exit 35
fi

api_source="$stage_dir/buglogin-api"
admin_source="$stage_dir/buglogin-admin-password-reset"
openapi_source="$stage_dir/openapi-v1.yaml"
for path in "$api_source" "$admin_source" "$openapi_source" "$stage_dir/deploy-api-systemd-remote.sh"; do
  if [[ -L "$path" || ! -f "$path" ]]; then
    echo "release input is missing or not a regular file" >&2
    exit 36
  fi
done
api_sha=$(sha256sum -- "$api_source" | cut -d ' ' -f 1)
admin_sha=$(sha256sum -- "$admin_source" | cut -d ' ' -f 1)
openapi_sha=$(sha256sum -- "$openapi_source" | cut -d ' ' -f 1)
if [[ "$api_sha" != "$expected_api_sha" || "$admin_sha" != "$expected_admin_sha" || "$openapi_sha" != "$expected_openapi_sha" ]]; then
  echo "staged release checksum mismatch" >&2
  exit 37
fi

unit_path=$(systemctl show "$unit" -p FragmentPath --value)
exec_path=$(systemctl show "$unit" -p ExecStart --value)
if [[ "$unit_path" != /etc/systemd/system/buglogin3-api.service || "$exec_path" != *"/opt/buglogin3/buglogin-api"* ]]; then
  echo "systemd unit does not match the BugLogin-only target" >&2
  exit 38
fi
if [[ "$(systemctl is-active "$unit")" != active || ! -f "$api_target" || -L "$api_target" ]]; then
  echo "BugLogin API is not active or its current executable is invalid" >&2
  exit 39
fi

install -d -o root -g buglogin3 -m 0750 "$rollback_dir"
stamp=$(date -u +%Y%m%dT%H%M%SZ)
nonce=$(od -An -N8 -tx1 /dev/urandom | tr -d ' \n')
deployment_id="$stamp-$release_commit-$nonce"
old_api_sha=$(sha256sum -- "$api_target" | cut -d ' ' -f 1)
old_api_backup="$rollback_dir/buglogin-api-$deployment_id-$old_api_sha"
cp -p -- "$api_target" "$old_api_backup"
old_admin_present=false
old_admin_sha=""
old_admin_backup=""
if [[ -e "$admin_target" ]]; then
  if [[ ! -f "$admin_target" || -L "$admin_target" ]]; then
    echo "existing admin tool path is invalid" >&2
    exit 40
  fi
  old_admin_sha=$(sha256sum -- "$admin_target" | cut -d ' ' -f 1)
  old_admin_backup="$rollback_dir/buglogin-admin-password-reset-$deployment_id-$old_admin_sha"
  cp -p -- "$admin_target" "$old_admin_backup"
  old_admin_present=true
fi
old_marker_present=false
old_marker_deployment_id=""
old_marker_api_sha=""
old_marker_admin_sha=""
if [[ -e "$active_marker" ]]; then
  if [[ -L "$active_marker" || ! -f "$active_marker" ]] || ! read_state "$active_marker"; then
    echo "existing active deployment marker is invalid; refusing activation" >&2
    exit 47
  fi
  old_marker_deployment_id=${state[deployment_id]:-}
  old_marker_api_sha=${state[api_sha256]:-}
  old_marker_admin_sha=${state[admin_sha256]:-}
  current_admin_sha=""
  if [[ -f "$admin_target" && ! -L "$admin_target" ]]; then
    current_admin_sha=$(sha256sum -- "$admin_target" | cut -d ' ' -f 1)
  fi
  if ! is_deployment_id "$old_marker_deployment_id" || "$old_marker_api_sha" != "$old_api_sha" || "$old_marker_admin_sha" != "$current_admin_sha"; then
    echo "active deployment marker does not match installed BugLogin binaries; refusing activation" >&2
    exit 48
  fi
  old_marker_present=true
fi

api_new="$api_target.new-$deployment_id"
admin_new="$admin_target.new-$deployment_id"
install -o root -g buglogin3 -m 0750 -- "$api_source" "$api_new"
install -o root -g buglogin3 -m 0750 -- "$admin_source" "$admin_new"
if ! mv -fT -- "$api_new" "$api_target"; then
  echo "could not activate the new API executable" >&2
  exit 41
fi
if ! mv -fT -- "$admin_new" "$admin_target"; then
  echo "could not activate the admin-reset executable; restoring the previous API" >&2
  restore_previous "$old_api_backup" "$old_admin_present" "$old_admin_backup" "$deployment_id" || true
  exit 42
fi
if ! systemctl restart "$unit"; then
  restore_previous "$old_api_backup" "$old_admin_present" "$old_admin_backup" "$deployment_id" || true
  exit 43
fi

ready=false
for _ in $(seq 1 30); do
  if systemctl is-active --quiet "$unit" && curl --fail --silent --max-time 2 http://127.0.0.1:12343/v1/readyz | grep -Eq '"status"[[:space:]]*:[[:space:]]*"ready"'; then
    ready=true
    break
  fi
  sleep 1
done
live_openapi_sha=$(curl --fail --silent --show-error --max-time 8 http://127.0.0.1:12343/v1/openapi.yaml | sha256sum | cut -d ' ' -f 1 || true)
if [[ "$ready" != true || "$live_openapi_sha" != "$expected_openapi_sha" ]]; then
  echo "post-deploy readiness or OpenAPI parity failed; restoring previous API" >&2
  restore_previous "$old_api_backup" "$old_admin_present" "$old_admin_backup" "$deployment_id" || true
  exit 44
fi
runtime_catalog=$(curl --silent --show-error --max-time 5 -D - -o /dev/null http://127.0.0.1:12343/v1/readyz | awk 'tolower($1)=="x-buglogin-runtime-catalog:" {gsub("\r", "", $2); print $2}' | tail -n 1)
if [[ "$runtime_catalog" != available ]]; then
  echo "signed runtime catalog is unavailable or its readiness header is missing; restoring previous API" >&2
  restore_previous "$old_api_backup" "$old_admin_present" "$old_admin_backup" "$deployment_id" || true
  exit 45
fi

state_file="$rollback_dir/deployment-$deployment_id.state"
state_tmp="$state_file.tmp.$$"
if ! {
  printf 'state=active\ndeployment_id=%s\nrelease_commit=%s\ndeployed_api_sha256=%s\ndeployed_admin_sha256=%s\nold_api_sha256=%s\nold_api_backup=%s\nold_admin_present=%s\nold_admin_sha256=%s\nold_admin_backup=%s\nold_marker_present=%s\nold_marker_deployment_id=%s\nold_marker_api_sha256=%s\nold_marker_admin_sha256=%s\n' \
    "$deployment_id" "$release_commit" "$expected_api_sha" "$expected_admin_sha" "$old_api_sha" "$old_api_backup" "$old_admin_present" "$old_admin_sha" "$old_admin_backup" "$old_marker_present" "$old_marker_deployment_id" "$old_marker_api_sha" "$old_marker_admin_sha" \
    > "$state_tmp" && chown root:buglogin3 "$state_tmp" && chmod 0640 "$state_tmp" && mv -fT -- "$state_tmp" "$state_file"
}; then
  echo "could not write deployment rollback receipt; restoring predecessor" >&2
  restore_previous "$old_api_backup" "$old_admin_present" "$old_admin_backup" "$deployment_id" || true
  rm -f -- "$state_tmp" "$state_file"
  exit 46
fi
if ! {
  printf '{"deploymentId":"%s","commit":"%s","apiSha256":"%s","adminResetSha256":"%s","openApiSha256":"%s","runtimeCatalog":"%s","deployedAtUtc":"%s"}\n' \
    "$deployment_id" "$release_commit" "$expected_api_sha" "$expected_admin_sha" "$live_openapi_sha" "$runtime_catalog" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
    > "$rollback_dir/deploy-$deployment_id.json" && chown root:buglogin3 "$rollback_dir/deploy-$deployment_id.json" && chmod 0640 "$rollback_dir/deploy-$deployment_id.json"
}; then
  echo "could not write deployment log; restoring predecessor" >&2
  restore_previous "$old_api_backup" "$old_admin_present" "$old_admin_backup" "$deployment_id" || true
  rm -f -- "$state_file" "$rollback_dir/deploy-$deployment_id.json"
  exit 46
fi
if ! write_marker "$deployment_id" "$expected_api_sha" "$expected_admin_sha"; then
  echo "could not record the active deployment marker; restoring the predecessor" >&2
  restore_previous "$old_api_backup" "$old_admin_present" "$old_admin_backup" "$deployment_id" || true
  if [[ "$old_marker_present" == true ]]; then
    write_marker "$old_marker_deployment_id" "$old_marker_api_sha" "$old_marker_admin_sha" || true
  else
    rm -f -- "$active_marker"
  fi
  rm -f -- "$state_file" "$rollback_dir/deploy-$deployment_id.json"
  exit 46
fi

for path in "$api_source" "$admin_source" "$openapi_source" "$stage_dir/deploy-api-systemd-remote.sh"; do
  rm -f -- "$path" || true
done
rmdir -- "$stage_dir" || true
echo "deployment_id=$deployment_id"
echo "deployment=active commit=$release_commit api_sha256=$expected_api_sha admin_reset_sha256=$expected_admin_sha openapi_sha256=$live_openapi_sha runtime_catalog=$runtime_catalog"
