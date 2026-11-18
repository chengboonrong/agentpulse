import { test } from '@e2e-dev/mobile';
import { expect } from 'e2e';

test('a phone keeps the Macs list and opens a Mac, then a session, one at a time', async ({ device, screen }) => {
  await device.installApp(undefined, { app: 'io.github.chengboonrong.agentpulse', reinstall: true });
  await device.openApp('io.github.chengboonrong.agentpulse', {
    relaunch: true,
    launchArguments: ['-e', 'fixture', 'macs'],
  });
  await device.setOrientation('portrait');

  await expect(screen.getByText('MacBook Pro')).toBeVisible();
  await screen.getByText('Studio').tap();
  await expect(screen.getByText('Sessions')).toBeVisible();
  await expect(screen.getByText('MacBook Pro')).not.toBeVisible();
  await screen.getByText('agentpulse').tap();
  await expect(screen.getByText('Running for')).toBeVisible();
  await expect(screen.getByText('Sessions')).not.toBeVisible();
  await device.back();
  await expect(screen.getByText('Sessions')).toBeVisible();
});

test('a rotation keeps the Pair step: the code check must not start over', async ({ device, screen }) => {
  await device.installApp(undefined, { app: 'io.github.chengboonrong.agentpulse', reinstall: true });
  await device.openApp('io.github.chengboonrong.agentpulse', {
    relaunch: true,
    launchArguments: ['-e', 'fixture', 'empty'],
    permissions: { camera: 'grant' },
  });
  await device.setOrientation('portrait');
  const b64 = (n: number, byte: number) => Buffer.alloc(n, byte).toString('base64url');
  const link = `agentpulse://pair?v=1&r=https://relay.example.com&id=${b64(16, 1)}&k=${b64(32, 9)}&s=${b64(32, 2)}&n=Studio`;

  await screen.getByText('Pair a Mac').tap();
  await screen.getByText('Paste the link instead').tap();
  await screen.getByRole('textbox').fill(link);
  await screen.getByText('Continue').tap();
  await expect(screen.getByText('Pair with Studio?')).toBeVisible();
  await device.setOrientation('landscape-left');
  await expect(screen.getByText('Pair with Studio?')).toBeVisible();
  await device.setOrientation('portrait');
});

test('Pair opens on the camera, and the link can be pasted instead', async ({ device, screen }) => {
  await device.installApp(undefined, { app: 'io.github.chengboonrong.agentpulse', reinstall: true });
  await device.openApp('io.github.chengboonrong.agentpulse', {
    relaunch: true,
    launchArguments: ['-e', 'fixture', 'empty'],
    permissions: { camera: 'grant' },
  });
  await device.setOrientation('portrait');

  await screen.getByText('Pair a Mac').tap();
  await expect(screen.getByText('Point the camera at the code on your Mac.')).toBeVisible();
  await screen.getByText('Paste the link instead').tap();
  await expect(screen.getByRole('textbox')).toBeVisible();
  // A rotation keeps pasting.
  await device.setOrientation('landscape-left');
  await expect(screen.getByRole('textbox')).toBeVisible();
  await device.setOrientation('portrait');
  await screen.getByText('Scan the code instead').tap();
  await expect(screen.getByText('Point the camera at the code on your Mac.')).toBeVisible();
});
