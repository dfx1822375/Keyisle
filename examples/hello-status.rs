#[cfg(windows)]
fn main() {
    use windows::{
        Security::Credentials::UI::UserConsentVerifier,
        Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize},
    };
    unsafe {
        RoInitialize(RO_INIT_MULTITHREADED).expect("initialize WinRT");
        let result = UserConsentVerifier::CheckAvailabilityAsync().and_then(|op| op.join());
        RoUninitialize();
        match result {
            Ok(value) => println!(
                "Windows Hello availability: {} (0=Available, 1=NoDevice, 2=NotConfigured, 3=Disabled, 4=Busy)",
                value.0
            ),
            Err(_) => {
                println!(
                    "Windows Hello availability check failed; master password fallback required"
                );
                std::process::exit(1);
            }
        }
    }
}
#[cfg(not(windows))]
fn main() {
    eprintln!("Windows only");
}
