#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]

#[cfg(feature = "generate-bindings")]
pub mod limelight {
    include!(concat!(env!("OUT_DIR"), "/limelight.rs"));
}
#[cfg(not(feature = "generate-bindings"))]
pub mod limelight;

#[cfg(all(feature = "generate-bindings", feature = "crypto"))]
mod crypto {
    include!(concat!(env!("OUT_DIR"), "/crypto.rs"));
}
#[cfg(all(not(feature = "generate-bindings"), feature = "crypto"))]
pub mod crypto;

/// Upstream moonlight-common-c's value for HIGH_AUDIO_BITRATE_THRESHOLD (kbps).
pub const UPSTREAM_HIGH_AUDIO_BITRATE_THRESHOLD: i32 = 15000;

#[cfg(moonlight_audio_threshold_patched)]
unsafe extern "C" {
    // Defined by the patched copy of moonlight-common-c built in build.rs.
    static mut MlHighAudioBitrateThreshold: std::os::raw::c_int;
}

/// Sets the video bitrate (kbps) at or above which high quality audio is
/// requested; 0 always requests it. Must be called before LiStartConnection.
/// Returns false when the linked library wasn't built from the patched source
/// (e.g. MOONLIGHT_COMMON_LIB), in which case upstream's fixed value applies.
pub fn set_high_audio_bitrate_threshold(kbps: i32) -> bool {
    #[cfg(moonlight_audio_threshold_patched)]
    {
        // SAFETY: only read by moonlight-common-c inside LiStartConnection, which
        // callers invoke after this on the same connection-setup path.
        unsafe { MlHighAudioBitrateThreshold = kbps };
        true
    }
    #[cfg(not(moonlight_audio_threshold_patched))]
    {
        let _ = kbps;
        false
    }
}
