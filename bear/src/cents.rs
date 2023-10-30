use num_traits::ToPrimitive;

pub fn cents_display(amt: i64) -> String {
    return (amt.to_f32().unwrap() / 100.0).to_string();
}

pub struct Cents {}

impl Cents {
    // maybe not clean because it doesn't return Self as per convention
    pub fn from(amt: f32) -> i64 {
        (amt * 100.0).round() as i64
    }

    pub fn to_float(amt: i64) -> f32 {
        amt.to_f32().unwrap() / 100.0
    }
}
