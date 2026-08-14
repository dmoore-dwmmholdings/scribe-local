module.exports = function (api) {
  api.cache(true);
  return {
    presets: ['babel-preset-expo'],
    // react-native-worklets/plugin powers Reanimated v4 worklets (used by the
    // Skia ember-vortex animations).  MUST be listed last.
    plugins: ['react-native-worklets/plugin'],
  };
};
