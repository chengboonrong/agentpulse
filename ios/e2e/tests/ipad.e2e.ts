import { test } from '@e2e-dev/mobile';
import { expect } from 'e2e';

const bundleId = 'io.github.chengboonrong.AgentPulse.phone';

test('portrait shows the first Mac beside an empty session column', async ({ device, screen }) => {
  await device.installApp();
  await device.setOrientation('portrait');
  await device.openApp(bundleId, { relaunch: true, launchArguments: ['-AgentPulseFixture', 'macs'] });

  await expect(screen.getByText('Studio')).toBeVisible();
  await expect(screen.getByText('No Session Selected')).toBeVisible();
});

test('landscape shows the Macs, the Mac and the chosen session side by side', async ({ device, screen }) => {
  await device.installApp();
  await device.openApp(bundleId, { relaunch: true, launchArguments: ['-AgentPulseFixture', 'macs'] });
  await device.setOrientation('landscape-left');

  await expect(screen.getByText('MacBook Pro')).toBeVisible();
  await screen.getByText('agentpulse').tap();
  await expect(screen.getByText('Running for')).toBeVisible();

  // Another Mac clears the session column.
  await screen.getByText('MacBook Pro').tap();
  await expect(screen.getByText('No live sessions')).toBeVisible();
  await expect(screen.getByText('No Session Selected')).toBeVisible();

  await device.setOrientation('portrait');
});
