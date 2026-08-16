/**
 * Expo config plugin: work around the fmt 11.0.2 / Xcode 16.3+ build failure.
 *
 * React Native 0.76 vendors its own fmt podspec pinned to 11.0.2
 * (node_modules/react-native/third-party-podspecs/fmt.podspec).  Compiled with
 * the clang in Xcode 16.3 and later, fmt 11.0.2's `consteval` FMT_STRING
 * constructor is rejected:
 *
 *     error: call to consteval function
 *     'fmt::basic_format_string<...>::basic_format_string<FMT_COMPILE_STRING, 0>'
 *     is not a constant expression
 *
 * Upstream fixed this in fmt 11.1, but the version is pinned inside
 * node_modules, so it cannot be bumped from this project durably.
 *
 * Instead we force `FMT_USE_CONSTEVAL 0` in the vendored copy — the same thing
 * fmt itself does on toolchains where consteval is known-broken (see the
 * `__apple_build_version__ < 14000029L` branch right below the one we patch).
 * The only consequence is losing fmt's compile-time format-string checking
 * inside fmt/folly; there is no runtime behaviour change.
 *
 * The patch is applied from the Podfile's post_install hook, so it survives
 * `pod install`.  This plugin injects that hook, so it survives
 * `expo prebuild` regenerating the Podfile.
 */

const { withDangerousMod } = require('expo/config-plugins');
const fs = require('fs');
const path = require('path');

const MARKER = 'fmt-consteval-fix';

const RUBY_HOOK = `
    # ${MARKER} — see mobile/plugins/withFmtConstevalFix.js
    fmt_base_h = File.join(installer.sandbox.root, 'fmt', 'include', 'fmt', 'base.h')
    if File.exist?(fmt_base_h)
      fmt_src = File.read(fmt_base_h)
      fmt_needle = '#if !defined(__cpp_lib_is_constant_evaluated)'
      if fmt_src.include?(fmt_needle)
        fmt_src = fmt_src.sub(fmt_needle, '#if 1 || !defined(__cpp_lib_is_constant_evaluated)')
        File.write(fmt_base_h, fmt_src)
        Pod::UI.puts '[${MARKER}] forced FMT_USE_CONSTEVAL 0 in fmt/base.h'
      end
    end
`;

module.exports = function withFmtConstevalFix(config) {
  return withDangerousMod(config, [
    'ios',
    (cfg) => {
      const podfilePath = path.join(cfg.modRequest.platformProjectRoot, 'Podfile');
      const podfile = fs.readFileSync(podfilePath, 'utf8');

      if (podfile.includes(MARKER)) {
        return cfg;
      }

      const anchor = /^([ \t]*)post_install do \|installer\|\n/m;
      if (!anchor.test(podfile)) {
        throw new Error(
          'withFmtConstevalFix: could not find `post_install do |installer|` in the Podfile.',
        );
      }

      fs.writeFileSync(
        podfilePath,
        podfile.replace(anchor, (match) => `${match}${RUBY_HOOK}`),
      );
      return cfg;
    },
  ]);
};
