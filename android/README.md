# WordCraft on Android (tablet)

The interface is the Rust app `apps/wordcraft-tablet` (eframe on Android). `android/` is the
thin Gradle shell around it: a `NativeActivity` that loads `libwordcraft_tablet.so` and handles the
system file picker, print and clipboard calls.

Build outputs and the native library are not committed (see `.gitignore`), so build them first.

## Requirements

- Rust with the target `aarch64-linux-android`, and `cargo-ndk`
- Android SDK with NDK 27.x and build-tools
- JDK 17 or later (Android Studio's bundled JBR works)
- `android/local.properties` with the SDK path, e.g. `sdk.dir=C\:/Users/you/AppData/Local/Android/Sdk`

## Build and install

From the repository root:

```sh
# 1. Native library (arm64-v8a only)
cargo ndk -t arm64-v8a build -p wordcraft-tablet --release
cp target/aarch64-linux-android/release/libwordcraft_tablet.so \
   android/app/src/main/jniLibs/arm64-v8a/

# 2. APK
cd android && ./gradlew assembleDebug && cd ..

# 3. Install and launch on a connected tablet
adb install -r android/app/build/outputs/apk/debug/app-debug.apk
adb shell am start -n wordcraft.android/.WordCraftActivity
```

Step 1 must be repeated after every change to the Rust code; Gradle only packages the library it finds
in `jniLibs/`. Logs go to logcat under the tag `wordcraft`.
