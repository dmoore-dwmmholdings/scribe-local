Pod::Spec.new do |s|
  s.name           = 'ScribeDiscovery'
  s.version        = '1.0.0'
  s.summary        = 'Finds Scribe servers on the local network over Bonjour'
  s.description    = 'Browses for _scribe._tcp with NWBrowser and reads the TXT record the server advertises, so the app can pair without anyone reading a URL off the server terminal.'
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
