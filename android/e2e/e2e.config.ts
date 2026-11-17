import type { E2EConfig } from 'e2e';
import { mobile } from '@e2e-dev/mobile';

// The debug APK from android/README.md; each test installs it, then opens a debug
// fixture (`am start -e fixture <name>`), so no Mac, relay or model is needed.
const app = {
  bundleId: 'io.github.chengboonrong.agentpulse',
  appPath: '../app/build/outputs/apk/debug/app-debug.apk',
};

export default {
  targets: [
    { name: 'tablet', engine: mobile({ platform: 'android', device: 'AgentPulse_Tablet' }), app },
    { name: 'phone', engine: mobile({ platform: 'android', device: 'Pixel_9a' }), app },
  ],
  workers: 1,
} satisfies E2EConfig;
