//! Independent simulator fixtures derived from Cobalt's existing public profiles.
//! No firmware payload, DTB, trace capture or vendor executable is embedded.
//! The digest pins only Cobalt's own selected identity fields, and never
//! authenticates a firmware image or establishes a newly measured device.

use kobo_profile::DeviceProfile;
#[cfg(test)]
use kobo_profile::SUPPORTED_PROFILES;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Board {
    pub profile_id: &'static str,
    pub serial_prefix: &'static str,
    pub device_code: u16,
    pub firmware: &'static str,
    pub kernel_release: &'static str,
    pub digest: &'static str,
    pub wifi_interface: &'static str,
}

// This table is authored from the existing in-repository Cobalt profile
// fields, not extracted from firmware. Wi-Fi interfaces are synthetic fixture
// choices for state-machine testing, not observations about these boards.
const BOARDS: &[Board] = &[
    Board {
        profile_id: "clara-hd-376",
        serial_prefix: "N249",
        device_code: 376,
        firmware: "4.38.23684",
        kernel_release: "4.1.15-00136-g12655eaaef89",
        digest: "9500869775494f8fb3b8d2cf0bb29c611eb5f81817ffc0240e3c9cd5b777d410",
        wifi_interface: "wlan0",
    },
    Board {
        profile_id: "libra-h2o-384",
        serial_prefix: "N873",
        device_code: 384,
        firmware: "4.38.23697",
        kernel_release: "4.1.15-00417-g0c800cffe1f9",
        digest: "ab75d8b39086eda010c1a162efd152cf2e52afc288625156373135cbcde7d392",
        wifi_interface: "mlan0",
    },
    Board {
        profile_id: "libra-2-388",
        serial_prefix: "N418",
        device_code: 388,
        firmware: "4.38.23697",
        kernel_release: "4.1.15-00868-g58a2758be07",
        digest: "e4c35b5eb57fd8ade4f2acaa5f4ec909f17aacf3103186aab6d84b5ddf395b33",
        wifi_interface: "wlan0",
    },
];

pub(super) fn for_profile(profile: &DeviceProfile) -> Option<&'static Board> {
    BOARDS.iter().find(|board| board.profile_id == profile.id)
}

pub(super) fn metadata(profile: &DeviceProfile) -> kobo_json::Value {
    let board = for_profile(profile);
    kobo_json::ObjectBuilder::new()
        .set("origin", "Cobalt profile-derived test fixture")
        .set("firmwarePayloadIncluded", false)
        .set("profile", profile.id)
        .set(
            "pinnedIdentitySha256",
            board.map_or(kobo_json::Value::Null, |b| b.digest.into()),
        )
        .set(
            "fixtureWifiInterface",
            board.map_or(kobo_json::Value::Null, |b| b.wifi_interface.into()),
        )
        .set("interfaceMeasured", false)
        .build()
}

#[cfg(test)]
fn identity(profile: &DeviceProfile, firmware: &str) -> String {
    format!(
        "{}\n{}\n{}\n{}\n{}\n",
        profile.id, profile.serial_prefix, profile.device_code, firmware, profile.kernel_release
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinned_board_identities_match_only_their_existing_profiles() {
        for board in BOARDS {
            let profile = SUPPORTED_PROFILES
                .iter()
                .copied()
                .find(|p| p.id == board.profile_id)
                .unwrap();
            assert_eq!(profile.serial_prefix, board.serial_prefix);
            assert_eq!(profile.device_code, board.device_code);
            assert_eq!(profile.kernel_release, board.kernel_release);
            assert!(profile.firmware_versions.contains(&board.firmware));
            assert_eq!(
                kobo_net::sha256::hex_digest(identity(profile, board.firmware).as_bytes()),
                board.digest
            );
            for other in SUPPORTED_PROFILES
                .iter()
                .copied()
                .filter(|p| p.id != board.profile_id)
            {
                assert_ne!(
                    kobo_net::sha256::hex_digest(identity(other, board.firmware).as_bytes()),
                    board.digest
                );
            }
        }
    }
}
