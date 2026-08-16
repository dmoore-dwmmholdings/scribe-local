# Install Scribe on an iPhone

Scribe has no App Store release. Thus you must build the app and install it on
the iPhone. This document gives two methods.

**Method A - Xcode with a personal Apple ID.**

- Cost: 0 USD.
- The app operates for 7 days.
- You must have a MacBook and a USB cable.

**Method B - Apple Developer Program.**

- Cost: 99 USD each year.
- The app operates for 1 year.
- You must have an Apple Developer account.

Method A is sufficient to test the app. Method B is better for daily
operation, because you install the app one time.

The two methods build the same app. Only the signature is different.

---

## Method A - Xcode with a personal Apple ID

### Before you start

Prepare these items on the MacBook:

- Xcode from the Mac App Store. Xcode is large, thus start this download first.
- Node, the current version. The command `brew install node` is sufficient.
- Git.
- Your personal Apple ID. An Apple Developer account is not necessary.
- The USB cable for the iPhone.

### 1. Get the code on the MacBook

```bash
git clone https://github.com/dmoore-dwmmholdings/scribe-local.git
cd scribe-local/mobile
npm install
```

### 2. Make the iOS project

```bash
npx expo prebuild --platform ios
```

This command makes the `ios` directory from `app.json`. It also installs the
CocoaPods dependencies. The `ios` directory is not in Git, because this command
makes it again at all times.

### 3. Open the project in Xcode

```bash
open ios/*.xcworkspace
```

Open the `.xcworkspace` file and not the `.xcodeproj` file. The `.xcodeproj`
file does not include the CocoaPods dependencies.

### 4. Give your Apple ID to Xcode

1. Open the `Xcode` menu, then `Settings`, then `Accounts`.
2. Push the `+` button and write your Apple ID.
3. Close the window.

### 5. Set the signature

1. In the left panel of Xcode, select the project at the top.
2. Select the target with the name `scribemobile`.
3. Select the `Signing & Capabilities` tab.
4. Set `Automatically manage signing` to on.
5. In the `Team` list, select your personal team. The name of this team
   includes the words `Personal Team`.
6. If Xcode shows an error about the bundle identifier, change the bundle
   identifier. Use a name that no other person uses, for example
   `com.dawson.scribe`. Write the same name in `app.json`, at the key
   `expo.ios.bundleIdentifier`.

### 6. Prepare the iPhone

1. Connect the iPhone to the MacBook with the USB cable.
2. On the iPhone, tap `Trust` for this computer. Then write the passcode.
3. On the iPhone, open `Settings`, then `Privacy & Security`, then
   `Developer Mode`.
4. Set `Developer Mode` to on. The iPhone starts again.

### 7. Select the release configuration

This step is important. Without it, the app cannot start by itself. It looks
for the Metro server on a computer.

1. Open the `Product` menu, then `Scheme`, then `Edit Scheme`.
2. Select `Run` in the left panel.
3. Set `Build Configuration` to `Release`.
4. Push `Close`.

The release configuration puts the JavaScript code in the app. Thus the app
starts without a computer.

### 8. Install the app

1. At the top of the Xcode window, select your iPhone as the destination.
2. Push the play button, or open the `Product` menu and select `Run`.
3. Wait 10 minutes or more for the first build.

### 9. Give approval to the certificate

The first start of the app shows the message `Untrusted Developer`. This is
usual for a personal Apple ID.

1. On the iPhone, open `Settings`, then `General`, then
   `VPN & Device Management`.
2. Select your Apple ID, then select `Trust`.
3. Start the app again.

### After 7 days

A certificate from a personal Apple ID operates for 7 days. Then the app does
not start. To continue:

1. Connect the iPhone to the MacBook.
2. Do step 8 again.

This makes a new certificate. Your recordings and your data stay in the app. A
personal Apple ID lets you install a maximum of 3 apps at the same time.

---

## Method B - Apple Developer Program

This method has a cost of 99 USD each year. But the app operates for 1 year
after one installation. The MacBook is not necessary. The build occurs in the
cloud, and the iPhone gets the app through the network.

1. Enroll at <https://developer.apple.com/programs>. Apple gives approval in 24
   hours to 48 hours.
2. On the Windows computer, in the `mobile` directory:

```bash
npx eas-cli@latest login
npx eas-cli@latest init
npx eas-cli@latest device:create
npx eas-cli@latest build --platform ios --profile preview
```

3. The command `device:create` shows a link. Open this link on the iPhone and
   install the profile. This step gives the identifier of your iPhone to Apple.
4. The command `build` tells you to give your Apple ID. It then makes the
   certificate and the provisioning profile. Answer `yes` to the two questions.
5. Wait 15 minutes to 25 minutes. Then the build page shows a QR code. Scan
   this QR code with the iPhone and install the app.

After 1 year, do step 4 and step 5 again.

---

## After the installation

Do these steps one time, for the two methods:

1. Install the Tailscale app on the iPhone. Sign in and connect to the tailnet.
2. On the Windows computer, start the backend with `.\scripts\launch-all.ps1`.
3. Open Scribe on the iPhone and give the microphone permission.
4. Open the `Settings` tab. Put the URL of the server in `Server URL`. The URL
   has this form: `https://<node-name>.ts.net`. The command `tailscale status`
   on the Windows computer shows the name of the node.
5. Keep `Device key` empty. In `deploy/local.toml`, the key
   `require_device_token` is `false`. Thus the API accepts all clients on the
   tailnet.
6. Tap `Save settings`, then tap `Test connection`.

The app is a capture client. The backend must operate on the Windows computer.
If the backend stops, the app cannot record, transcribe, or find recordings.

iOS refuses connections that are not HTTPS. The command `tailscale serve` gives
a correct certificate to the API. Thus the HTTPS URL operates, but a URL with
`http://` does not operate.

---

## New versions of the app

Method A and method B put the JavaScript code in the app. Thus a change to the
code makes a new build necessary.

- Method A: do step 8 again.
- Method B: do step 5 again.

The `expo-updates` package can send changes in the JavaScript code to the app
through the network. Then a new build is necessary only for a change of a
native module. Scribe does not have this package at this time.

---

## Problems and solutions

| Problem | Solution |
|---|---|
| `Untrusted Developer` at the first start | Do step 9. |
| The app stops after 7 days | Do step 8 again. |
| `expo prebuild` stops at CocoaPods | Install CocoaPods with `brew install cocoapods`. Then do step 2 again. |
| Xcode shows a bundle identifier error | Do step 5, item 6. |
| The app starts, but shows no recordings | Examine Tailscale on the iPhone. Then examine the URL of the server in the app. |
| `Test connection` fails | Make sure that the backend operates. Open `http://127.0.0.1:8443/health` on the Windows computer. |
| The app cannot start without a computer | The build used the debug configuration. Do step 7, then do step 8 again. |
