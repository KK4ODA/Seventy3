# Changelog

All notable changes to Seventy3 are listed here. The release workflow copies
each version's section into its GitHub release and into the signed update
manifest, so it is what the in-app "What's new" shows.

## [Unreleased]

### Added

- Quick Start Guide (PDF) in `docs/`, attached to every release.

## [0.1.0] - 2026-11-22

First release. Tested against the built-in TS-480 simulator; not yet tested
on a real TS-480 and RTL-SDR.

### Added

- Panadapter and waterfall of the TS-480's 73.095 MHz first IF from an
  RTL-SDR, centred on the dial, with zoom from about 1.6 MHz down to 30 kHz.
- The waterfall stays aligned to RF as you tune.
- CAT control over the TS-480's COM port: frequency, mode, band memory, and
  split, RIT and XIT display.
- Click, drag, mouse-wheel and keyboard tuning; SSB clicks centre the signal
  in the passband.
- Transmit from the app: PTT button or hold Space, Esc always unkeys, a
  transmit timeout, and unkey on exit or crash. The waterfall pauses while
  transmitting.
- POTA activator spots, listed and marked on the panadapter; click to tune.
- Calibration against a carrier on a known frequency.
- Demo mode with a simulated TS-480 and band.
- Signed automatic updates from GitHub releases.
