#![forbid(unsafe_code)]

pub fn get_bundled_json(translation: &str, book_id: &str, chapter: u32) -> Option<&'static str> {
    if !translation.eq_ignore_ascii_case("BSB") {
        return None;
    }

    if book_id.eq_ignore_ascii_case("MRK") {
        match chapter {
            1 => Some(include_str!("../bundled/BSB/MRK/1.json")),
            2 => Some(include_str!("../bundled/BSB/MRK/2.json")),
            3 => Some(include_str!("../bundled/BSB/MRK/3.json")),
            4 => Some(include_str!("../bundled/BSB/MRK/4.json")),
            5 => Some(include_str!("../bundled/BSB/MRK/5.json")),
            6 => Some(include_str!("../bundled/BSB/MRK/6.json")),
            7 => Some(include_str!("../bundled/BSB/MRK/7.json")),
            8 => Some(include_str!("../bundled/BSB/MRK/8.json")),
            9 => Some(include_str!("../bundled/BSB/MRK/9.json")),
            10 => Some(include_str!("../bundled/BSB/MRK/10.json")),
            11 => Some(include_str!("../bundled/BSB/MRK/11.json")),
            12 => Some(include_str!("../bundled/BSB/MRK/12.json")),
            13 => Some(include_str!("../bundled/BSB/MRK/13.json")),
            14 => Some(include_str!("../bundled/BSB/MRK/14.json")),
            15 => Some(include_str!("../bundled/BSB/MRK/15.json")),
            16 => Some(include_str!("../bundled/BSB/MRK/16.json")),
            _ => None,
        }
    } else if book_id.eq_ignore_ascii_case("GEN") {
        match chapter {
            1 => Some(include_str!("../bundled/BSB/GEN/1.json")),
            2 => Some(include_str!("../bundled/BSB/GEN/2.json")),
            3 => Some(include_str!("../bundled/BSB/GEN/3.json")),
            _ => None,
        }
    } else if book_id.eq_ignore_ascii_case("JHN") {
        match chapter {
            1 => Some(include_str!("../bundled/BSB/JHN/1.json")),
            2 => Some(include_str!("../bundled/BSB/JHN/2.json")),
            3 => Some(include_str!("../bundled/BSB/JHN/3.json")),
            _ => None,
        }
    } else if book_id.eq_ignore_ascii_case("PSA") {
        match chapter {
            23 => Some(include_str!("../bundled/BSB/PSA/23.json")),
            _ => None,
        }
    } else if book_id.eq_ignore_ascii_case("PRO") {
        match chapter {
            3 => Some(include_str!("../bundled/BSB/PRO/3.json")),
            _ => None,
        }
    } else {
        None
    }
}
