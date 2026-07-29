fn main() {
    // The release workflow supplies this public identifier at compile time.  Tracking the
    // environment dependency ensures a rebuilt package cannot reuse a stale embedded ID.
    println!("cargo:rerun-if-env-changed=AGY_BUNDLED_GOOGLE_OAUTH_CLIENT_ID");
    tauri_build::build()
}
