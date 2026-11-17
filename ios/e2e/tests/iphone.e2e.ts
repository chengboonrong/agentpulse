import { test } from '@e2e-dev/mobile';
import { expect } from 'e2e';

test('iPhone keeps the Macs list and pushes a Mac', async ({ device, screen }) => {
  await device.installApp();
  await device.setOrientation('portrait');
  await device.openApp('io.github.chengboonrong.AgentPulse.phone', {
    relaunch: true,
    launchArguments: ['-AgentPulseFixture', 'macs'],
  });

  await expect(screen.getByText('MacBook Pro')).toBeVisible();
  await screen.getByText('Studio').tap();
  await expect(screen.getByText('Sessions')).toBeVisible();
  await expect(screen.getByText('No Session Selected')).not.toBeVisible();
});
