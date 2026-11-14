//! "Battery low" and "fully charged" alerts for the other device, shared by the laptop
//! and the phone so both follow the same rules.
//!
//! * Low: at or under [`LOW`]% and not charging. Once per discharge: it re-arms only when
//!   the level is back over [`REARM`]% or the charger is plugged in.
//! * Full: 100% while charging, once per charge, and only after this charge was seen below
//!   100% (so reconnecting to an already full device doesn't announce it again).

pub const LOW: u8 = 15;
pub const REARM: u8 = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatteryAlert {
    Low(u8),
    Full,
}

/// Per-device memory of what was already announced.
#[derive(Debug, Clone, Copy, Default)]
pub struct BatteryAlerts {
    low_sent: bool,
    full_armed: bool,
    full_sent: bool,
}

impl BatteryAlerts {
    /// Feed the latest reading; returns an alert when one is due.
    pub fn update(&mut self, battery: Option<u8>, charging: bool) -> Option<BatteryAlert> {
        let b = battery.filter(|b| *b <= 100)?;
        if charging || b > REARM {
            self.low_sent = false;
        }
        if !charging {
            self.full_armed = false;
            self.full_sent = false;
        } else if b < 100 && !self.full_sent {
            self.full_armed = true;
        }
        if !charging && b <= LOW && !self.low_sent {
            self.low_sent = true;
            return Some(BatteryAlert::Low(b));
        }
        if charging && b >= 100 && self.full_armed {
            self.full_armed = false;
            self.full_sent = true;
            return Some(BatteryAlert::Full);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{BatteryAlert::*, BatteryAlerts};

    fn run(steps: &[(Option<u8>, bool)]) -> Vec<Option<super::BatteryAlert>> {
        let mut a = BatteryAlerts::default();
        steps.iter().map(|(b, c)| a.update(*b, *c)).collect()
    }

    #[test]
    fn low_once_per_discharge() {
        let got = run(&[
            (Some(40), false),
            (Some(16), false),
            (Some(15), false), // crosses: alert
            (Some(12), false), // no repeat
            (Some(18), false), // not re-armed yet (≤ 20)
            (Some(14), false),
            (Some(21), false), // re-armed
            (Some(15), false), // alert again
        ]);
        assert_eq!(got, [None, None, Some(Low(15)), None, None, None, None, Some(Low(15))]);
    }

    #[test]
    fn charging_rearms_and_never_warns_low() {
        let got = run(&[(Some(10), false), (Some(10), true), (Some(9), true), (Some(9), false)]);
        assert_eq!(got, [Some(Low(10)), None, None, Some(Low(9))]);
    }

    #[test]
    fn full_once_per_charge_and_only_after_charging_below_full() {
        let got = run(&[
            (Some(100), true), // already full when first seen: quiet
            (Some(100), true),
            (Some(99), false),
            (Some(98), true),
            (Some(100), true), // alert
            (Some(100), true), // once
            (Some(99), true),  // hovering at full while plugged in: still once
            (Some(100), true),
            (Some(97), false), // unplugged
            (Some(97), true),
            (Some(100), true), // a new charge: alert
        ]);
        assert_eq!(got, [None, None, None, None, Some(Full), None, None, None, None, None, Some(Full)]);
    }

    #[test]
    fn unknown_battery_does_nothing() {
        assert_eq!(run(&[(None, false), (Some(200), false)]), [None, None]);
    }
}
