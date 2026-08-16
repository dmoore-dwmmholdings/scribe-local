Pod::Spec.new do |s|
  s.name           = 'ScribeBgTimer'
  s.version        = '1.0.0'
  s.summary        = 'Background-safe repeating timer for segment rotation'
  s.description    = 'A repeating timer backed by DispatchSourceTimer, which keeps firing while the app runs in the background with the screen locked — unlike React Native JS timers, which are driven by a CADisplayLink that halts.'
  s.author         = ''
  s.homepage       = 'https://github.com/dmoore-dwmmholdings/scribe-local'
  s.platforms      = { :ios => '15.1' }
  s.source         = { git: '' }
  s.static_framework = true

  s.dependency 'ExpoModulesCore'

  s.pod_target_xcconfig = {
    'DEFINES_MODULE' => 'YES',
    'SWIFT_COMPILATION_MODE' => 'wholemodule'
  }

  s.source_files = "**/*.{h,m,mm,swift,hpp,cpp}"
end
