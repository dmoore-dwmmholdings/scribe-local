Pod::Spec.new do |s|
  s.name           = 'ScribeAudioSession'
  s.version        = '1.0.0'
  s.summary        = 'Reports AVAudioSession interruptions to JS'
  s.description    = 'Surfaces AVAudioSession interruption and media-reset notifications so a recording can resume itself instead of silently stopping.'
  s.author         = ''
  s.homepage       = 'https://github.com/dmoore-dwmmholdings/scribe-local'
  s.platforms      = { :ios => '15.1' }
  s.source         = { git: '' }
  s.static_framework = true

  s.dependency 'ExpoModulesCore'
  s.frameworks = 'AVFoundation'

  s.pod_target_xcconfig = {
    'DEFINES_MODULE' => 'YES',
    'SWIFT_COMPILATION_MODE' => 'wholemodule'
  }

  s.source_files = "**/*.{h,m,mm,swift,hpp,cpp}"
end
