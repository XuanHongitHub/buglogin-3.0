export type UpdatePreparationRequest = {
  source: "manual" | "background";
  updateAvailable: boolean;
  consented: boolean;
  autoUpdatesEnabled: boolean;
  isBusy: boolean;
  updateReady: boolean;
};

export function shouldPrepareAppUpdate({
  source,
  updateAvailable,
  consented,
  autoUpdatesEnabled,
  isBusy,
  updateReady,
}: UpdatePreparationRequest): boolean {
  if (!updateAvailable || isBusy || updateReady) return false;
  if (source === "manual") return consented;
  return autoUpdatesEnabled;
}
