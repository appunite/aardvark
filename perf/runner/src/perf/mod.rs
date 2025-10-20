use super::LoadProfile;

macro_rules! embed {
    ($path:literal) => {
        include_str!($path).to_string()
    };
}

pub fn echo_json(profile: LoadProfile) -> String {
    match profile {
        LoadProfile::None => embed!("../../../fixtures/scenarios/echo_none.py"),
        LoadProfile::Low => embed!("../../../fixtures/scenarios/echo_low.py"),
        LoadProfile::Medium => embed!("../../../fixtures/scenarios/echo_medium.py"),
        LoadProfile::High => embed!("../../../fixtures/scenarios/echo_high.py"),
    }
}

pub fn echo_rawctx(profile: LoadProfile) -> String {
    match profile {
        LoadProfile::None => embed!("../../../fixtures/scenarios/echo_rawctx_none.py"),
        LoadProfile::Low => embed!("../../../fixtures/scenarios/echo_rawctx_low.py"),
        LoadProfile::Medium => embed!("../../../fixtures/scenarios/echo_rawctx_medium.py"),
        LoadProfile::High => embed!("../../../fixtures/scenarios/echo_rawctx_high.py"),
    }
}

pub fn numpy_json(profile: LoadProfile) -> String {
    match profile {
        LoadProfile::None => embed!("../../../fixtures/scenarios/numpy_none.py"),
        LoadProfile::Low => embed!("../../../fixtures/scenarios/numpy_low.py"),
        LoadProfile::Medium => embed!("../../../fixtures/scenarios/numpy_medium.py"),
        LoadProfile::High => embed!("../../../fixtures/scenarios/numpy_high.py"),
    }
}

pub fn numpy_rawctx(profile: LoadProfile) -> String {
    match profile {
        LoadProfile::None => embed!("../../../fixtures/scenarios/numpy_rawctx_none.py"),
        LoadProfile::Low => embed!("../../../fixtures/scenarios/numpy_rawctx_low.py"),
        LoadProfile::Medium => embed!("../../../fixtures/scenarios/numpy_rawctx_medium.py"),
        LoadProfile::High => embed!("../../../fixtures/scenarios/numpy_rawctx_high.py"),
    }
}

pub fn pandas_json(profile: LoadProfile) -> String {
    match profile {
        LoadProfile::None => embed!("../../../fixtures/scenarios/pandas_none.py"),
        LoadProfile::Low => embed!("../../../fixtures/scenarios/pandas_low.py"),
        LoadProfile::Medium => embed!("../../../fixtures/scenarios/pandas_medium.py"),
        LoadProfile::High => embed!("../../../fixtures/scenarios/pandas_high.py"),
    }
}

pub fn pandas_rawctx(profile: LoadProfile) -> String {
    match profile {
        LoadProfile::None => embed!("../../../fixtures/scenarios/pandas_rawctx_none.py"),
        LoadProfile::Low => embed!("../../../fixtures/scenarios/pandas_rawctx_low.py"),
        LoadProfile::Medium => embed!("../../../fixtures/scenarios/pandas_rawctx_medium.py"),
        LoadProfile::High => embed!("../../../fixtures/scenarios/pandas_rawctx_high.py"),
    }
}
