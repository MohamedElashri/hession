// Consent rules are independent of displaying or exporting a recovery key.
export function canEnableDeviceSync(settings, { previewValid, createdHere, recoverySaved, fingerprintConfirmed }) {
  return !!settings?.configured && !settings.setupPending && !!previewValid
    && (createdHere ? !!recoverySaved : !!fingerprintConfirmed);
}

export function canDisconnectDeviceSync(settings, { recoveryVisible, recoverySaved }) {
  return !!(settings?.configured || settings?.setupPending) && !settings.enabled && !settings.status?.running
    && (!settings.setupPending || (!!recoveryVisible && !!recoverySaved));
}
