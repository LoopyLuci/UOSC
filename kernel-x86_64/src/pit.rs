//! Real PIT (8253/8254) programming — using `uosc_core::timer::pit_divisor`,
//! the exact function ported from `drivers/timer.ti` and covered by
//! `reference-rs`'s property tests, to compute the divisor, then writing it
//! to the real hardware ports (0x43 control, 0x40 channel 0 data). This is
//! the first place in this whole effort where the "pure arithmetic, no
//! hardware" Phase-0 timer port actually drives real hardware — everything
//! up to this point only ran under `cargo test`.

use x86_64::instructions::port::Port;

const PIT_CONTROL_PORT: u16 = 0x43;
const PIT_CHANNEL0_PORT: u16 = 0x40;
const PIT_CHANNEL0_SQUARE_WAVE: u8 = 0x36; // channel 0, lobyte/hibyte, mode 3

/// Reprograms channel 0 to fire at `freq_hz`, using the real, tested
/// divisor arithmetic instead of trusting the PIT's ~18.2 Hz power-on
/// default.
pub fn set_frequency(freq_hz: u32) {
    let divisor = uosc_core::timer::pit_divisor(freq_hz)
        .expect("pit_divisor rejected this frequency — see uosc_core::timer for why");

    let mut control: Port<u8> = Port::new(PIT_CONTROL_PORT);
    let mut data: Port<u8> = Port::new(PIT_CHANNEL0_PORT);
    unsafe {
        control.write(PIT_CHANNEL0_SQUARE_WAVE);
        data.write((divisor & 0xFF) as u8);
        data.write((divisor >> 8) as u8);
    }
}
