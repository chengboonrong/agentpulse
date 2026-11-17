import { test } from '@e2e-dev/mobile';
import { expect } from 'e2e';

const pkg = 'io.github.chengboonrong.agentpulse';
const macs = ['-e', 'fixture', 'macs'];
// agent-device names rotations from a phone's point of view: `portrait` is rotation 0, the
// natural orientation, which on the Pixel Tablet is landscape.
const portrait = 'landscape-left';
const landscape = 'portrait';

test('portrait shows the Macs beside the first Mac', async ({ device, screen }) => {
  await device.installApp(undefined, { app: 'io.github.chengboonrong.agentpulse', reinstall: true });
  await device.openApp(pkg, { relaunch: true, launchArguments: macs });
  await device.setOrientation(portrait);

  await expect(screen.getByText('MacBook Pro')).toBeVisible();
  await expect(screen.getByText('Needs you')).toBeVisible();
  // Two panes: choosing a session puts it beside the Mac, the Macs go.
  await screen.getByText('agentpulse').tap();
  await expect(screen.getByText('Running for')).toBeVisible();
  await expect(screen.getByText('Needs you')).toBeVisible();
  await expect(screen.getByText('MacBook Pro')).not.toBeVisible();
});

test('landscape shows the Macs, the Mac and the chosen session side by side', async ({ device, screen }) => {
  await device.installApp(undefined, { app: 'io.github.chengboonrong.agentpulse', reinstall: true });
  await device.openApp(pkg, { relaunch: true, launchArguments: macs });
  await device.setOrientation(landscape);

  await expect(screen.getByText('No Session Selected')).toBeVisible();
  await screen.getByText('agentpulse').tap();
  await expect(screen.getByText('Running for')).toBeVisible();
  await expect(screen.getByText('MacBook Pro')).toBeVisible();

  // Another Mac clears the session pane.
  await screen.getByText('MacBook Pro').tap();
  await expect(screen.getByText('No live sessions')).toBeVisible();
  await expect(screen.getByText('No Session Selected')).toBeVisible();

  await device.setOrientation(portrait);
});
