import test from 'node:test';
import assert from 'node:assert/strict';
import { canEnableDeviceSync, canDisconnectDeviceSync } from './device-sync-settings.js';

test('creating a library requires reviewed scope and a saved recovery key', () => {
  const configured = { configured: true };
  const consent = { createdHere: true, previewValid: true, recoverySaved: true };
  assert.equal(canEnableDeviceSync(configured, consent), true);
  assert.equal(canEnableDeviceSync(configured, { ...consent, previewValid: false }), false);
  assert.equal(canEnableDeviceSync(configured, { ...consent, recoverySaved: false }), false);
  assert.equal(canEnableDeviceSync({ configured: false }, consent), false);
  assert.equal(canEnableDeviceSync({ ...configured, setupPending: true }, consent), false);
});

test('revealing a key never bypasses fingerprint confirmation for a joined device', () => {
  const settings = { configured: true };
  const consent = { createdHere: false, previewValid: true, recoverySaved: true, fingerprintConfirmed: false };
  assert.equal(canEnableDeviceSync(settings, consent), false);
  assert.equal(canEnableDeviceSync(settings, { ...consent, fingerprintConfirmed: true }), true);
  assert.equal(canEnableDeviceSync(settings, { ...consent, fingerprintConfirmed: true, previewValid: false }), false);
});

test('an interrupted setup cannot discard its only saved key before recovery', () => {
  const pending = { setupPending: true, enabled: false, status: { running: false } };
  assert.equal(canDisconnectDeviceSync(pending, { recoverySaved: true, recoveryVisible: false }), false);
  assert.equal(canDisconnectDeviceSync(pending, { recoverySaved: false, recoveryVisible: true }), false);
  assert.equal(canDisconnectDeviceSync(pending, { recoverySaved: true, recoveryVisible: true }), true);
  assert.equal(canDisconnectDeviceSync({ ...pending, enabled: true }, { recoverySaved: true, recoveryVisible: true }), false);
  assert.equal(canDisconnectDeviceSync({ ...pending, status: { running: true } }, { recoverySaved: true, recoveryVisible: true }), false);
  assert.equal(canDisconnectDeviceSync({ configured: true, enabled: false }, {}), true);
});
