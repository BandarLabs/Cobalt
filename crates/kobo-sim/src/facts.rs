//! Hardware facts about each simulated reader that the panel profile does not
//! record.
//!
//! A profile describes the panel and digitiser, because that is what Cobalt
//! has measured and writes to. Applications also meet the rest of the reader:
//! whether there are page-turn buttons to press, and which Wi-Fi networks the
//! radio can hear at all. A simulator that gives every profile the same
//! buttons and the same networks lets an app ship having relied on a button a
//! Clara does not have, or on a 5 GHz network a Libra 2 cannot see.
//!
//! These come from Kobo's published specifications and the firmware packages,
//! not from attended measurement, so they shape only what the simulator
//! offers. Nothing on a device reads them.

/// Which Wi-Fi bands the radio can join.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Bands {
    /// 802.11 b/g/n only. The reader never sees a 5 GHz network.
    TwoPointFour,
    /// 802.11 a/b/g/n/ac across 2.4 and 5 GHz.
    Dual,
}

impl Bands {
    pub const fn name(self) -> &'static str {
        match self {
            Self::TwoPointFour => "2.4 GHz",
            Self::Dual => "2.4 and 5 GHz",
        }
    }

    pub const fn hears(self, band: Band) -> bool {
        matches!((self, band), (Self::Dual, _) | (_, Band::TwoPointFour))
    }
}

/// The band one simulated network broadcasts on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Band {
    TwoPointFour,
    Five,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Facts {
    /// Whether the reader has physical page-turn buttons.
    pub page_keys: bool,
    pub wifi_bands: Bands,
}

/// The facts for a profile, or `None` for one this table does not know.
///
/// Matched by profile ID so that a new profile without an entry fails the
/// coverage test below instead of quietly inheriting another reader's
/// buttons. The Clara BW P365 (`clara-bw-395`) shares the N365's retail
/// specification but has its own firmware package and board, so its entry
/// repeats the retail facts rather than pointing at the N365's.
pub fn facts(profile_id: &str) -> Option<Facts> {
    let (page_keys, wifi_bands) = match profile_id {
        "clara-bw-391" | "clara-bw-395" | "clara-colour-393" | "elipsa-2e-389" => {
            (false, Bands::Dual)
        }
        "clara-hd-376" => (false, Bands::TwoPointFour),
        "libra-h2o-384" | "libra-2-388" => (true, Bands::TwoPointFour),
        "libra-colour-390" | "libra-colour-390-4.46.23836" => (true, Bands::Dual),
        _ => return None,
    };
    Some(Facts {
        page_keys,
        wifi_bands,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_supported_profile_states_its_hardware_facts() {
        for profile in kobo_profile::SUPPORTED_PROFILES {
            assert!(
                facts(profile.id).is_some(),
                "{} needs an entry in the simulator's hardware facts",
                profile.id
            );
        }
    }

    #[test]
    fn only_the_libras_have_page_turn_buttons() {
        for profile in kobo_profile::SUPPORTED_PROFILES {
            assert_eq!(
                facts(profile.id).unwrap().page_keys,
                profile.id.starts_with("libra-"),
                "{}",
                profile.id
            );
        }
    }

    #[test]
    fn a_two_point_four_gigahertz_radio_never_hears_a_five_gigahertz_network() {
        assert!(!Bands::TwoPointFour.hears(Band::Five));
        assert!(Bands::TwoPointFour.hears(Band::TwoPointFour));
        assert!(Bands::Dual.hears(Band::Five));
        assert_eq!(
            facts("libra-2-388").unwrap().wifi_bands,
            Bands::TwoPointFour
        );
    }
}
