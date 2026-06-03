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

pub fn tensor_script() -> &'static str {
    include_str!("../../../fixtures/scenarios/tensor.py")
}

pub fn matplotlib_script() -> &'static str {
    include_str!("../../../fixtures/scenarios/matplotlib.py")
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

pub fn matplotlib_points(profile: LoadProfile) -> Option<u64> {
    match profile {
        LoadProfile::None => None,
        LoadProfile::Low => Some(parse_u64(include_str!(
            "../../../fixtures/inputs/matplotlib_low.txt"
        ))),
        LoadProfile::Medium => Some(parse_u64(include_str!(
            "../../../fixtures/inputs/matplotlib_medium.txt"
        ))),
        LoadProfile::High => Some(parse_u64(include_str!(
            "../../../fixtures/inputs/matplotlib_high.txt"
        ))),
    }
}

pub fn tensor_length(profile: LoadProfile) -> usize {
    match profile {
        LoadProfile::None => 0,
        LoadProfile::Low => 256,
        LoadProfile::Medium => 16_384,
        LoadProfile::High => 262_144,
    }
}

pub fn tensor_data(profile: LoadProfile) -> Vec<f32> {
    let len = tensor_length(profile);
    (0..len)
        .map(|index| {
            let x = index as f32 * 0.001953125; // deterministic but non-trivial signal
            (x.sin() + x.cos()) * 0.5
        })
        .collect()
}

pub fn tensor_bytes(profile: LoadProfile) -> Vec<u8> {
    let mut buffer = Vec::new();
    for value in tensor_data(profile) {
        buffer.extend_from_slice(&value.to_le_bytes());
    }
    buffer
}

fn parse_u64(text: &str) -> u64 {
    text.trim()
        .parse::<u64>()
        .expect("fixture values must parse as integers")
}
