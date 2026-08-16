Pod::Spec.new do |s|
  s.name           = 'ScribeLiveActivity'
  s.version        = '1.0.0'
  s.summary        = 'Drives the recording Live Activity from the app process'
  s.description    = 'Starts, updates and ends the ActivityKit Live Activity that shows recording status on the Lock Screen and in the Dynamic Island.'
  s.author         = ''
  s.homepage       = 'https://github.com/dmoore-dwmmholdings/scribe-local'
  s.platforms      = { :ios => '15.1' }
  s.source         = { git: '' }
  s.static_framework = true

  s.dependency 'ExpoModulesCore'

  # The app deploys to 15.1 but ActivityKit only exists from 16.1, so it must be
  # weak-linked; every call site is behind `if #available(iOS 16.2, *)`.
  s.weak_frameworks = 'ActivityKit'

  s.pod_target_xcconfig = {
    'DEFINES_MODULE' => 'YES',
    'SWIFT_COMPILATION_MODE' => 'wholemodule'
  }

  s.source_files = "**/*.{h,m,mm,swift,hpp,cpp}"
end
