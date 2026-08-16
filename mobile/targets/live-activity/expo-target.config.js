/**
 * Live Activity widget extension.
 *
 * Expo prebuild cannot create additional Xcode targets, so @bacons/apple-targets
 * generates this one from the directory. Everything in this folder is compiled
 * into the extension, NOT into the main app.
 *
 * No App Groups entitlement is requested: the Live Activity is started and
 * updated directly from the app process via ActivityKit, so there is no shared
 * storage to coordinate. That also keeps the target signable with a free
 * personal team, which cannot sign App Groups.
 *
 * @type {import('@bacons/apple-targets/app.plugin').ConfigFunction}
 */
module.exports = () => ({
  type: 'widget',
  // Must NOT collide with the ScribeLiveActivity pod (the app-side native
  // module). Both become Swift module names, and the app — which deploys to
  // 15.1 — would otherwise resolve `import ScribeLiveActivity` to this
  // target's swiftmodule, built for 16.2, and fail to compile.
  name: 'ScribeWidgets',
  // Leading dot => appended to the main app's bundle identifier.
  bundleIdentifier: '.widgets',
  // Live Activities require iOS 16.1; 16.2 is the floor for the
  // ActivityContent/staleDate API used here. The app itself still targets 15.1.
  deploymentTarget: '16.2',
});
