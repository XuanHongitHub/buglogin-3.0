"use client";

import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  LuKeyRound,
  LuLogOut,
  LuRefreshCw,
  LuShield,
  LuUserRound,
  LuX,
} from "react-icons/lu";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { createTauriDesktopApi } from "../../../apps/desktop/src/bridge";
import type {
  AccountIdentity,
  WebSession,
} from "../../../frontend/src/generated/command-registry";
import { AccountError, accountError } from "./accounts";
import "./accounts-i18n";

const api = createTauriDesktopApi();

export function PersonalAccount() {
  const { t } = useTranslation("managementAccounts");
  const [identity, setIdentity] = useState<AccountIdentity | null>(null);
  const [sessions, setSessions] = useState<WebSession[]>([]);
  const [values, setValues] = useState({
    username: "",
    displayName: "",
    email: "",
    currentPassword: "",
  });
  const [password, setPassword] = useState({
    current: "",
    next: "",
    confirm: "",
  });
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");

  const reload = useCallback(async () => {
    setLoading(true);
    setError("");
    try {
      const [profile, sessionPage] = await Promise.all([
        api.getOwnAccountIdentity(),
        api.listAccountSessions("me"),
      ]);
      setIdentity(profile);
      setSessions(sessionPage.sessions);
      setValues({
        username: profile.username,
        displayName: profile.displayName ?? "",
        email: profile.email,
        currentPassword: "",
      });
    } catch (reason) {
      setError(accountError(reason));
    } finally {
      setLoading(false);
    }
  }, []);
  useEffect(() => {
    void reload();
  }, [reload]);

  async function saveIdentity(event: React.FormEvent) {
    event.preventDefault();
    if (!identity) return;
    setSaving(true);
    setError("");
    setNotice("");
    try {
      const updated = await api.updateOwnAccountIdentity({
        ...values,
        accountId: identity.accountId,
        expectedRevision: identity.revision,
        currentPassword: values.currentPassword || undefined,
      });
      setIdentity(updated);
      setValues({
        username: updated.username,
        displayName: updated.displayName ?? "",
        email: updated.email,
        currentPassword: "",
      });
      setNotice(t("saved"));
    } catch (reason) {
      setError(accountError(reason));
    } finally {
      setSaving(false);
    }
  }
  async function changePassword(event: React.FormEvent) {
    event.preventDefault();
    setError("");
    setNotice("");
    if (password.next !== password.confirm) {
      setError(t("passwordMismatch"));
      return;
    }
    setSaving(true);
    try {
      await api.changeOwnAccountPassword({
        currentPassword: password.current,
        newPassword: password.next,
      });
      setPassword({ current: "", next: "", confirm: "" });
      setSessions([]);
      setNotice(t("passwordChanged"));
    } catch (reason) {
      setError(accountError(reason));
    } finally {
      setSaving(false);
    }
  }
  async function revoke(sessionId: string) {
    setSaving(true);
    setError("");
    try {
      await api.revokeAccountSession("me", sessionId);
      await reload();
      setNotice(t("signedOut"));
    } catch (reason) {
      setError(accountError(reason));
    } finally {
      setSaving(false);
    }
  }

  return (
    <section className="flex min-h-0 flex-1 flex-col overflow-hidden">
      <header className="flex shrink-0 items-center justify-between border-b border-border px-4 py-3">
        <div>
          <h1 className="text-base font-semibold">{t("personal")}</h1>
          <p className="mt-0.5 text-xs text-muted-foreground">
            {t("policyHint")}
          </p>
        </div>
        <Button
          size="icon"
          variant="ghost"
          aria-label={t("refresh")}
          onClick={() => void reload()}
          disabled={loading}
        >
          <LuRefreshCw className={loading ? "animate-spin" : ""} />
        </Button>
      </header>
      <div className="min-h-0 flex-1 overflow-auto p-4">
        <div className="mx-auto grid w-full max-w-5xl gap-4 xl:grid-cols-2">
          <form
            onSubmit={(event) => void saveIdentity(event)}
            className="space-y-4 rounded-md border border-border p-4"
          >
            <div className="flex items-center gap-2">
              <LuUserRound className="size-4 text-muted-foreground" />
              <h2 className="text-sm font-semibold">{t("ownInformation")}</h2>
              {identity ? (
                <Badge variant="secondary" className="ml-auto">
                  {identity.verification}
                </Badge>
              ) : null}
            </div>
            <AccountError message={error} />
            <div className="grid gap-3 sm:grid-cols-2">
              <div className="space-y-1.5">
                <Label htmlFor="personal-username">{t("username")}</Label>
                <Input
                  id="personal-username"
                  value={values.username}
                  onChange={(event) =>
                    setValues((current) => ({
                      ...current,
                      username: event.target.value,
                    }))
                  }
                  disabled={loading || saving}
                  autoComplete="username"
                />
              </div>
              <div className="space-y-1.5">
                <Label htmlFor="personal-display-name">
                  {t("displayName")}
                </Label>
                <Input
                  id="personal-display-name"
                  value={values.displayName}
                  onChange={(event) =>
                    setValues((current) => ({
                      ...current,
                      displayName: event.target.value,
                    }))
                  }
                  disabled={loading || saving}
                  autoComplete="name"
                />
              </div>
              <div className="space-y-1.5 sm:col-span-2">
                <Label htmlFor="personal-email">{t("email")}</Label>
                <Input
                  id="personal-email"
                  type="email"
                  value={values.email}
                  onChange={(event) =>
                    setValues((current) => ({
                      ...current,
                      email: event.target.value,
                    }))
                  }
                  disabled={loading || saving}
                  autoComplete="email"
                />
              </div>
              <div className="space-y-1.5 sm:col-span-2">
                <Label htmlFor="identity-current-password">
                  {t("currentPassword")}
                </Label>
                <Input
                  id="identity-current-password"
                  type="password"
                  value={values.currentPassword}
                  onChange={(event) =>
                    setValues((current) => ({
                      ...current,
                      currentPassword: event.target.value,
                    }))
                  }
                  disabled={loading || saving}
                  autoComplete="current-password"
                />
              </div>
            </div>
            <div className="flex items-center justify-between gap-3 border-t border-border pt-3">
              <span
                role="status"
                className="min-w-0 text-xs text-muted-foreground"
              >
                {notice}
              </span>
              <Button size="sm" disabled={loading || saving || !identity}>
                {t("save")}
              </Button>
            </div>
          </form>
          <form
            onSubmit={(event) => void changePassword(event)}
            className="space-y-4 rounded-md border border-border p-4"
          >
            <div className="flex items-center gap-2">
              <LuKeyRound className="size-4 text-muted-foreground" />
              <h2 className="text-sm font-semibold">{t("security")}</h2>
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="password-current">{t("currentPassword")}</Label>
              <Input
                id="password-current"
                type="password"
                value={password.current}
                onChange={(event) =>
                  setPassword((current) => ({
                    ...current,
                    current: event.target.value,
                  }))
                }
                autoComplete="current-password"
                disabled={saving}
                required
              />
            </div>
            <div className="grid gap-3 sm:grid-cols-2">
              <div className="space-y-1.5">
                <Label htmlFor="password-next">{t("newPassword")}</Label>
                <Input
                  id="password-next"
                  type="password"
                  minLength={12}
                  maxLength={256}
                  value={password.next}
                  onChange={(event) =>
                    setPassword((current) => ({
                      ...current,
                      next: event.target.value,
                    }))
                  }
                  autoComplete="new-password"
                  disabled={saving}
                  required
                />
              </div>
              <div className="space-y-1.5">
                <Label htmlFor="password-confirm">{t("confirmPassword")}</Label>
                <Input
                  id="password-confirm"
                  type="password"
                  minLength={12}
                  maxLength={256}
                  value={password.confirm}
                  onChange={(event) =>
                    setPassword((current) => ({
                      ...current,
                      confirm: event.target.value,
                    }))
                  }
                  autoComplete="new-password"
                  disabled={saving}
                  required
                />
              </div>
            </div>
            <p className="text-xs text-muted-foreground">
              {t("sessionRevocationHint")}
            </p>
            <div className="flex justify-end border-t border-border pt-3">
              <Button size="sm" disabled={saving}>
                {t("changePassword")}
              </Button>
            </div>
          </form>
          <section className="space-y-3 rounded-md border border-border p-4 xl:col-span-2">
            <div className="flex items-center gap-2">
              <LuShield className="size-4 text-muted-foreground" />
              <h2 className="text-sm font-semibold">{t("sessions")}</h2>
              <span className="ml-auto text-xs text-muted-foreground">
                {sessions.length}
              </span>
            </div>
            {sessions.length ? (
              <ul className="divide-y divide-border">
                {sessions.map((session) => (
                  <li
                    key={session.id}
                    className="flex items-center gap-3 py-2.5"
                  >
                    <span className="min-w-0 flex-1">
                      <span className="block text-sm font-medium">
                        {t(session.kind)}
                        {session.current ? ` · ${t("currentSession")}` : ""}
                      </span>
                      <span className="block text-xs text-muted-foreground">
                        {session.createdAt
                          ? new Date(session.createdAt).toLocaleString()
                          : t("deviceUnavailable")}
                      </span>
                    </span>
                    <Badge variant={session.current ? "secondary" : "outline"}>
                      {session.revokedAt ? t("revoked") : t("active")}
                    </Badge>
                    {!session.current && !session.revokedAt ? (
                      <Button
                        size="sm"
                        variant="outline"
                        onClick={() => void revoke(session.id)}
                        disabled={saving}
                      >
                        <LuX />
                        {t("signOut")}
                      </Button>
                    ) : null}
                  </li>
                ))}
              </ul>
            ) : (
              <p className="text-sm text-muted-foreground">
                {loading ? t("loading") : t("noSessions")}
              </p>
            )}
          </section>
          <div className="flex justify-end xl:col-span-2">
            <Button
              variant="outline"
              onClick={() => void api.logout()}
              disabled={saving}
            >
              <LuLogOut />
              {t("signOut")}
            </Button>
          </div>
        </div>
      </div>
    </section>
  );
}
