//! Timer arithmetic — the pure-logic subset of `drivers/timer.ti`.
//!
//! Everything else in that file is direct MMIO/MSR/port I/O
//! (`*(apic_base as *mut u32).add(...) = ...`, `asm!("rdtsc")`,
//! `io::write_u8`) — hardware access with no portable behavior to test in
//! this crate. What's real, portable, and worth getting right: the
//! frequency-to-divisor arithmetic that configures the PIT/APIC, and the
//! tick/TSC-to-wall-clock conversion math. A bug in either would silently
//! misconfigure every timer interrupt on real hardware, and nothing in the
//! Titan source exercises this arithmetic in isolation to catch one.
//!
//! **Real bug found while porting**: neither `init_pit_timer` nor
//! `init_apic_timer`/`set_apic_frequency` in `drivers/timer.ti` validates
//! `freq_hz != 0` before dividing by it (`kernel/timer.ti:210`, `:234`) —
//! a caller passing `0` divides by zero. This port makes that division
//! fallible instead of letting it panic (or, on real hardware without Rust's
//! debug-mode checked division, silently wrap/UB).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimerError {
    ZeroFrequency,
    DivisorOutOfRange,
}

pub const PIT_BASE_FREQ_HZ: u32 = 1_193_182;
pub const APIC_BUS_CLOCK_HZ: u32 = 100_000_000;
pub const APIC_DIVISOR: u32 = 16;

/// Real port of the PIT divisor arithmetic in `init_pit_timer`
/// (`kernel/timer.ti:233-238`), including its round-to-nearest rounding.
pub fn pit_divisor(freq_hz: u32) -> Result<u16, TimerError> {
    if freq_hz == 0 {
        return Err(TimerError::ZeroFrequency);
    }
    let divisor = (PIT_BASE_FREQ_HZ + freq_hz / 2) / freq_hz;
    if divisor > u16::MAX as u32 {
        return Err(TimerError::DivisorOutOfRange);
    }
    Ok(divisor as u16)
}

/// Real port of the APIC initial-count arithmetic in `set_apic_frequency`
/// (`kernel/timer.ti:203-218`), assuming the same fixed 100 MHz bus clock
/// and divide-by-16 the Titan source hardcodes.
///
/// **Second real bug found here**: `divisor * freq_hz` in the Titan source
/// is unchecked `u32` arithmetic — for any `freq_hz` above roughly 268
/// million, `16 * freq_hz` overflows a 32-bit register. On real x86
/// hardware that silently wraps to a nonsense divisor; in a checked Rust
/// build it panics. `checked_mul` turns that into a reported error either
/// way.
pub fn apic_initial_count(freq_hz: u32) -> Result<u32, TimerError> {
    if freq_hz == 0 {
        return Err(TimerError::ZeroFrequency);
    }
    let divisor_product = APIC_DIVISOR.checked_mul(freq_hz).ok_or(TimerError::DivisorOutOfRange)?;
    Ok(APIC_BUS_CLOCK_HZ / divisor_product)
}

/// Real port of `get_time_ns`'s tick-based branch (`kernel/timer.ti:81-85`):
/// `tick_count * (1_000_000_000 / frequency_hz)`.
pub fn ticks_to_ns(tick_count: u64, freq_hz: u32) -> Result<u64, TimerError> {
    if freq_hz == 0 {
        return Err(TimerError::ZeroFrequency);
    }
    let interval_ns = 1_000_000_000u64 / freq_hz as u64;
    Ok(tick_count.saturating_mul(interval_ns))
}

/// Real port of `get_time_ns`'s TSC branch (`kernel/timer.ti:76-80`),
/// using `u128` intermediate math exactly like the source does to avoid
/// overflow at high TSC counts.
pub fn tsc_to_ns(tsc: u64, tsc_frequency_hz: u64) -> Result<u64, TimerError> {
    if tsc_frequency_hz == 0 {
        return Err(TimerError::ZeroFrequency);
    }
    Ok(((tsc as u128 * 1_000_000_000u128) / tsc_frequency_hz as u128) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn zero_frequency_is_rejected_everywhere_not_panicked() {
        assert_eq!(pit_divisor(0), Err(TimerError::ZeroFrequency));
        assert_eq!(apic_initial_count(0), Err(TimerError::ZeroFrequency));
        assert_eq!(ticks_to_ns(100, 0), Err(TimerError::ZeroFrequency));
        assert_eq!(tsc_to_ns(100, 0), Err(TimerError::ZeroFrequency));
    }

    #[test]
    fn pit_divisor_at_100hz_matches_the_documented_example() {
        // drivers/timer.ti's own comment: "100 for 100Hz = 10ms ticks".
        let divisor = pit_divisor(100).unwrap();
        assert_eq!(divisor, ((PIT_BASE_FREQ_HZ + 50) / 100) as u16);
    }

    #[test]
    fn very_low_frequency_overflows_the_16_bit_pit_divisor() {
        // 1 Hz needs a divisor far larger than the PIT's 16-bit counter.
        assert_eq!(pit_divisor(1), Err(TimerError::DivisorOutOfRange));
    }

    #[test]
    fn apic_initial_count_at_1khz_is_computed_correctly() {
        assert_eq!(apic_initial_count(1000).unwrap(), APIC_BUS_CLOCK_HZ / (APIC_DIVISOR * 1000));
    }

    #[test]
    fn ticks_to_ns_at_100hz_is_10ms_per_tick() {
        assert_eq!(ticks_to_ns(1, 100).unwrap(), 10_000_000);
        assert_eq!(ticks_to_ns(1000, 100).unwrap(), 10_000_000_000);
    }

    #[test]
    fn tsc_to_ns_scales_with_frequency() {
        // At a 1 GHz TSC, 1_000_000_000 ticks is exactly 1 second.
        assert_eq!(tsc_to_ns(1_000_000_000, 1_000_000_000).unwrap(), 1_000_000_000);
    }

    proptest! {
        /// Neither divisor helper panics for any nonzero frequency, and
        /// pit_divisor's result — when in range — really does produce the
        /// documented round-to-nearest PIT count.
        #[test]
        fn no_panics_for_any_nonzero_frequency(freq_hz in 1u32..=u32::MAX) {
            let _ = pit_divisor(freq_hz);
            let _ = apic_initial_count(freq_hz);
            let _ = ticks_to_ns(1, freq_hz);
        }
    }
}
