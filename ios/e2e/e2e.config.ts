import type { E2EConfig } from 'e2e';
import { mobile } from '@e2e-dev/mobile';

// The Debug simulator build from ios/README.md; each test installs it, then opens
// a DEBUG fixture (-AgentPulseFixture), so no Mac, relay or model is needed.
const app = {
  bundleId: 'io.github.chengboonrong.AgentPulse.phone',
  appPath: '../../target/ios/Build/Products/Debug-iphonesimulator/AgentPulse.app',
};

export default {
  targets: [
    { name: 'ipad', engine: mobile({ platform: 'ios', device: 'iPad Pro 11-inch (M5)' }), app },
    { name: 'iphone', engine: mobile({ platform: 'ios', device: 'iPhone 18 Pro' }), app },
  ],
  workers: 1,
} satisfies E2EConfig;
