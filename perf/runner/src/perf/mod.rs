use super::LoadProfile;

const NUMPY_LOW_SIZE: u64 = 64;
const NUMPY_MEDIUM_SIZE: u64 = 4_096;
const NUMPY_HIGH_SIZE: u64 = 1_000_000;

const PANDAS_LOW_ROWS: u64 = 128;
const PANDAS_MEDIUM_ROWS: u64 = 10_000;
const PANDAS_HIGH_ROWS: u64 = 1_000_000;

const MATPLOTLIB_LOW_POINTS: u64 = 128;
const MATPLOTLIB_MEDIUM_POINTS: u64 = 4_096;
const MATPLOTLIB_HIGH_POINTS: u64 = 65_536;

pub fn echo_script() -> &'static str {
    include_str!("../../../fixtures/scenarios/echo.py")
}

pub fn numpy_script() -> &'static str {
    include_str!("../../../fixtures/scenarios/numpy.py")
}

pub fn numpy_matmul_script() -> &'static str {
    include_str!("../../../fixtures/scenarios/numpy_matmul.py")
}

pub fn pandas_script() -> &'static str {
    include_str!("../../../fixtures/scenarios/pandas.py")
}

pub fn scipy_sgemm_script() -> &'static str {
    include_str!("../../../fixtures/scenarios/scipy_sgemm.py")
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
        LoadProfile::Low => Some(NUMPY_LOW_SIZE),
        LoadProfile::Medium => Some(NUMPY_MEDIUM_SIZE),
        LoadProfile::High => Some(NUMPY_HIGH_SIZE),
    }
}

pub fn matrix_size(profile: LoadProfile) -> Option<u64> {
    match profile {
        LoadProfile::None => None,
        LoadProfile::Low => Some(64),
        LoadProfile::Medium => Some(128),
        LoadProfile::High => Some(256),
    }
}

pub fn pandas_rows(profile: LoadProfile) -> Option<u64> {
    match profile {
        LoadProfile::None => None,
        LoadProfile::Low => Some(PANDAS_LOW_ROWS),
        LoadProfile::Medium => Some(PANDAS_MEDIUM_ROWS),
        LoadProfile::High => Some(PANDAS_HIGH_ROWS),
    }
}

pub fn matplotlib_points(profile: LoadProfile) -> Option<u64> {
    match profile {
        LoadProfile::None => None,
        LoadProfile::Low => Some(MATPLOTLIB_LOW_POINTS),
        LoadProfile::Medium => Some(MATPLOTLIB_MEDIUM_POINTS),
        LoadProfile::High => Some(MATPLOTLIB_HIGH_POINTS),
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
