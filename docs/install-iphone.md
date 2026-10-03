# Install Scribe on an iPhone

Scribe has no App Store release, so you build the app and install it on the
iPhone yourself. The app is native SwiftUI, in [`ios/`](../ios/). Its Xcode
project is generated from `ios/project.yml` by XcodeGen and is not committed.

Two ways to sign it:

| | Personal Apple ID | Apple Developer Program |
|---|---|---|
| Cost | free | 99 USD a year |
| The app runs for | 7 days, then reinstall | 1 year |
| Needs | a Mac and a USB cable | a developer account |

Either builds the same app; only the signature differs.

## Before you start

On the Mac:

- Xcode, from the Mac App Store.
- XcodeGen: `brew install xcodegen`.
- Your Apple ID, signed in to Xcode (Xcode → Settings → Accounts).

On the iPhone: Developer Mode on (Settings → Privacy & Security → Developer
Mode). The setting appears after the iPhone has been connected to Xcode once.

## 1. Generate the project

```bash
git clone https://github.com/dmoore-dwmmholdings/scribe-local.git
cd scribe-local/ios
xcodegen
```

`project.yml` signs with the DWMM Holdings team (`P5GS55MLHL`) and bundle id
`com.dwmmholdings.scribenative`. For another team, change `DEVELOPMENT_TEAM`
and `PRODUCT_BUNDLE_IDENTIFIER` there and run `xcodegen` again. A bundle id is
unique across every Apple team, so pick one of your own; do not use
`com.dwmmholdings.scribe.*`, which another team holds.

## 2. Build and install

In Xcode: open `ios/Scribe.xcodeproj`, choose your iPhone as the destination,
and press Run.

From the command line:

```bash
xcodebuild -project Scribe.xcodeproj -scheme Scribe -configuration Release \
  -destination 'generic/platform=iOS' -derivedDataPath build/dd \
  -allowProvisioningUpdates build
xcrun devicectl list devices            # find the iPhone's identifier
xcrun devicectl device install app --device <identifier> \
  build/dd/Build/Products/Release-iphoneos/Scribe.app
```

With a personal Apple ID, the first launch needs the certificate trusted on the
iPhone: Settings → General → VPN & Device Management → your Apple ID → Trust.

## 3. Connect it to your server

Install Tailscale on the iPhone and sign in to the same tailnet as the server.
Then, in the app:

- **Scan the QR code** the installer printed, or open its `scribe://pair` link.
  That fills in the server and the device key in one step.
- Or open **Settings → Find server** while the iPhone is on the same Wi-Fi as
  the server. On a phone signed in to Tailscale as the server's owner, no
  device key is needed.

**Test connection** checks the server answers and accepts this phone.

## If it does not work

| Problem | Fix |
|---|---|
| `xcodegen: command not found` | `brew install xcodegen` |
| "No Accounts" or no team | Sign in to Xcode → Settings → Accounts, then build again. |
| "Failed Registering Bundle Identifier" | The bundle id is taken. Choose another in `project.yml`, then `xcodegen`. |
| "Build input file cannot be found: …mobileprovision" | The first build after a team or bundle-id change races the profile download. Build again. |
| The app does not open: "Untrusted Developer" | Trust the certificate (step 2). |
| Test connection: "Could not reach the server" | Connect Tailscale on the iPhone. |
| Find server finds nothing | Same Wi-Fi as the server, and the server installed with LAN discovery on. |
