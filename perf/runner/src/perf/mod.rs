use super::LoadProfile;

pub fn echo_script() -> &'static str {
    include_str!("../../../fixtures/scenarios/echo.py")
}

pub fn numpy_script() -> &'static str {
    include_str!("../../../fixtures/scenarios/numpy.py")
}

pub fn pandas_script() -> &'static str {
    include_str!("../../../fixtures/scenarios/pandas.py")
}

pub fn echo_payload(profile: LoadProfile) -> Option<&'static [u8]> {
    match profile {
        LoadProfile::None => None,
        LoadProfile::Low => Some(include_bytes!("../../../fixtures/inputs/echo_low.txt")),
        LoadProfile::Medium => Some(include_bytes!("../../../fixtures/inputs/echo_medium.txt")),
        LoadProfile::High => Some(include_bytes!("../../../fixtures/inputs/echo_high.txt")),
    }
}

pub fn numpy_size(profile: LoadProfile) -> Option<u64> {
    match profile {
        LoadProfile::None => None,
        LoadProfile::Low => Some(parse_u64(include_str!(
            "../../../fixtures/inputs/numpy_low.txt"
        ))),
        LoadProfile::Medium => Some(parse_u64(include_str!(
            "../../../fixtures/inputs/numpy_medium.txt"
        ))),
        LoadProfile::High => Some(parse_u64(include_str!(
            "../../../fixtures/inputs/numpy_high.txt"
        ))),
    }
}

pub fn pandas_rows(profile: LoadProfile) -> Option<u64> {
    match profile {
        LoadProfile::None => None,
        LoadProfile::Low => Some(parse_u64(include_str!(
            "../../../fixtures/inputs/pandas_low.txt"
        ))),
        LoadProfile::Medium => Some(parse_u64(include_str!(
            "../../../fixtures/inputs/pandas_medium.txt"
        ))),
        LoadProfile::High => Some(parse_u64(include_str!(
            "../../../fixtures/inputs/pandas_high.txt"
        ))),
    }
}

fn parse_u64(text: &str) -> u64 {
    text.trim()
        .parse::<u64>()
        .expect("fixture values must parse as integers")
}
