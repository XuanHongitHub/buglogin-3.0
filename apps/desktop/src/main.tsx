import { StrictMode, useState } from "react";
import { createRoot } from "react-dom/client";
import DesktopHome from "../../../src/app/desktop-home.js";
import { useAppUpdateNotifications } from "../../../src/hooks/use-app-update-notifications.js";
import type { BrowserProfile } from "../../../src/types.js";
import { AuthBootstrap } from "./AuthBootstrap.js";
import { createTauriDesktopApi } from "./bridge.js";
import { DonutDesktopProviders } from "./DonutDesktopProviders.js";
import { ProfileSharingDialog } from "./ProfileSharingDialog.js";
import "../../../src/styles/globals.css";

const root = document.getElementById("root");
if (!root) throw new Error("Desktop root element not found");

const api = createTauriDesktopApi();

function DesktopRoot() {
  const appUpdates = useAppUpdateNotifications();
  const [share, setShare] = useState<{
    profiles: BrowserProfile[];
    workspaceId: string;
  }>();
  return (
    <>
      <DesktopHome
        appUpdates={appUpdates}
        onCheckForUpdates={appUpdates.checkForAppUpdatesManual}
        isCheckingForUpdates={appUpdates.isCheckingForUpdates}
        onShareProfiles={(profiles, workspaceId) =>
          setShare({ profiles, workspaceId })
        }
      />
      {share ? (
        <ProfileSharingDialog
          api={api}
          profiles={share.profiles}
          workspaceId={share.workspaceId}
          open
          onOpenChange={(open) => {
            if (!open) setShare(undefined);
          }}
        />
      ) : null}
    </>
  );
}

createRoot(root).render(
  <StrictMode>
    <DonutDesktopProviders>
      <AuthBootstrap api={api}>
        <DesktopRoot />
      </AuthBootstrap>
    </DonutDesktopProviders>
  </StrictMode>,
);
