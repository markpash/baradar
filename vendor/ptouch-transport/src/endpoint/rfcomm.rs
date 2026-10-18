//! Platform-independent RFCOMM helpers shared by the macOS and Linux backends.
//!
//! - [`chunk_len`] / [`mtu_chunks`]: the zero-MTU rule of PROTOCOL.md §2.1 "Writes".
//! - [`channel_candidates`]: the §2.1 channel order (SDP, `rfcomm_channel_observed`, 1, 2).
//! - [`probe_channel`] / [`probe_status`]: channel acceptance; a candidate channel counts only
//!   after a valid `80 20 42` status reply to `00×100 1B 40 1B 69 53` within the read timeout,
//!   with up to [`PROBE_ATTEMPTS`] probes for a half-awake link (§2.1).
//! - [`is_printer_class_of_device`]: the Brother printer CoD test (§2.1 "Device
//!   identification").
//!
//! These are pure functions (except [`probe_status`], which only talks to a [`Transport`]),
//! so they are unit-tested on every platform.

use std::time::Duration;

use crate::{Transport, TransportError};

/// Bluetooth Serial Port Profile UUID16 (PROTOCOL.md §2.1).
pub const SPP_UUID16: u16 = 0x1101;

/// Conservative write chunk when the channel reports an MTU of 0 (measured PT-P710BT MTU).
pub const FALLBACK_CHUNK: usize = 320;

/// Read timeout for one channel-acceptance probe (PROTOCOL.md §2.1 "Wake-up": 5 s per
/// attempt).
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Probe attempts per channel before it counts as silent (PROTOCOL.md §2.1 "Wake-up and first
/// contact": a half-awake link may miss the first request; up to 3 attempts).
pub const PROBE_ATTEMPTS: u32 = 3;

/// Invalidate (100 × `00`) + `ESC @` + `ESC i S`: the channel-acceptance probe (PROTOCOL.md
/// §2.1, §6.1). Neither command prints or feeds.
#[must_use]
pub fn probe_bytes() -> Vec<u8> {
    let mut v = vec![0u8; 100];
    v.extend_from_slice(&[0x1B, 0x40, 0x1B, 0x69, 0x53]);
    v
}

/// The write chunk size for a channel MTU: `mtu`, or [`FALLBACK_CHUNK`] when the MTU reads 0.
/// Never 0, never more than `u16::MAX` (the `writeSync:length:` length type).
#[must_use]
pub fn chunk_len(mtu: usize) -> usize {
    match mtu {
        0 => FALLBACK_CHUNK,
        m => m.min(usize::from(u16::MAX)),
    }
}

/// Splits `bytes` into write chunks of at most [`chunk_len`]`(mtu)` bytes. Never yields an
/// empty chunk (an empty input yields nothing).
pub fn mtu_chunks(bytes: &[u8], mtu: usize) -> impl Iterator<Item = &[u8]> {
    bytes.chunks(chunk_len(mtu))
}

/// RFCOMM channel candidates in PROTOCOL.md §2.1 order: the SDP-resolved channel, then the
/// model's `rfcomm_channel_observed`, then 1, then 2; deduplicated, invalid channels (0, >30)
/// dropped.
#[must_use]
pub fn channel_candidates(sdp: Option<u8>, observed: Option<u8>) -> Vec<u8> {
    let mut out = Vec::with_capacity(4);
    for ch in [sdp, observed, Some(1), Some(2)].into_iter().flatten() {
        if (1..=30).contains(&ch) && !out.contains(&ch) {
            out.push(ch);
        }
    }
    out
}

/// `true` for Class of Device major 6 (Imaging) with the "printer" minor bit (IOBluetooth
/// minor value `0x20`), e.g. `0x042680` on the PT-P710BT.
#[must_use]
pub fn is_printer_class_of_device(cod: u32) -> bool {
    let major = (cod >> 8) & 0x1F;
    let minor = (cod >> 2) & 0x3F;
    major == 6 && minor & 0x20 != 0
}

/// `true` when `buf` contains a complete 32-byte frame starting with `80 20 42`.
#[must_use]
pub fn contains_status_frame(buf: &[u8]) -> bool {
    buf.windows(3)
        .enumerate()
        .any(|(i, w)| w == [0x80, 0x20, 0x42] && buf.len() - i >= 32)
}

/// Sends the probe ([`probe_bytes`]) and waits up to `timeout` for a valid status frame.
///
/// Returns `Ok(true)` if a frame arrived, `Ok(false)` on timeout. The frame is consumed (the
/// session performs its own handshake afterwards).
///
/// # Errors
/// Write/read failures of the transport.
pub fn probe_status(t: &mut dyn Transport, timeout: Duration) -> Result<bool, TransportError> {
    t.write_all(&probe_bytes())?;
    let deadline = crate::deadline_after(timeout);
    let mut acc: Vec<u8> = Vec::with_capacity(64);
    let mut buf = [0u8; 64];
    loop {
        let left = crate::time_left(deadline);
        if left.is_zero() {
            return Ok(false);
        }
        let n = t.read(&mut buf, left)?;
        acc.extend_from_slice(buf.get(..n).unwrap_or_default());
        if contains_status_frame(&acc) {
            // Drain whatever else is immediately available (e.g. a coalesced second frame).
            while t.read(&mut buf, Duration::from_millis(50))? > 0 {}
            return Ok(true);
        }
        if acc.len() > 4096 {
            // Garbage only: keep the tail that could still start a frame.
            let keep = acc.split_off(acc.len() - 31);
            acc = keep;
        }
    }
}

/// Channel acceptance (PROTOCOL.md §2.1): [`probe_status`] up to `attempts` times, re-sending
/// the probe each time, so that a half-awake printer on the right channel is not rejected as
/// silent after its first missed reply. `Ok(false)` only when every attempt timed out.
///
/// # Errors
/// Write/read failures of the transport (not retried).
pub fn probe_channel(
    t: &mut dyn Transport,
    attempts: u32,
    timeout: Duration,
) -> Result<bool, TransportError> {
    for _ in 0..attempts.max(1) {
        if probe_status(t, timeout)? {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{TransportInfo, TransportKind};

    /// Answers the `n`-th probe (1-based) with a status frame, ignores the others.
    struct Sleepy {
        answer_on: u32,
        probes: u32,
        inbox: Vec<u8>,
        info: TransportInfo,
    }

    impl Transport for Sleepy {
        fn write_all(&mut self, _bytes: &[u8]) -> Result<(), TransportError> {
            self.probes += 1;
            if self.probes == self.answer_on {
                let mut f = vec![0u8; 32];
                f[..3].copy_from_slice(&[0x80, 0x20, 0x42]);
                self.inbox = f;
            }
            Ok(())
        }
        fn read(&mut self, buf: &mut [u8], timeout: Duration) -> Result<usize, TransportError> {
            if self.inbox.is_empty() {
                std::thread::sleep(timeout.min(Duration::from_millis(5)));
                return Ok(0);
            }
            let n = buf.len().min(self.inbox.len());
            buf[..n].copy_from_slice(&self.inbox[..n]);
            self.inbox.drain(..n);
            Ok(n)
        }
        fn close(&mut self) -> Result<(), TransportError> {
            Ok(())
        }
        fn info(&self) -> &TransportInfo {
            &self.info
        }
    }

    fn sleepy(answer_on: u32) -> Sleepy {
        Sleepy {
            answer_on,
            probes: 0,
            inbox: Vec::new(),
            info: TransportInfo {
                kind: TransportKind::Virtual,
                label: "sleepy".into(),
                max_write: None,
            },
        }
    }

    #[test]
    fn half_awake_channel_is_accepted_on_a_later_attempt() {
        let short = Duration::from_millis(30);
        let mut t = sleepy(3);
        assert!(probe_channel(&mut t, PROBE_ATTEMPTS, short).unwrap());
        assert_eq!(t.probes, 3);
        let mut t = sleepy(4);
        assert!(!probe_channel(&mut t, PROBE_ATTEMPTS, short).unwrap());
        assert_eq!(t.probes, 3, "no more than PROBE_ATTEMPTS probes");
    }

    #[test]
    fn probe_accepts_any_timeout() {
        let mut t = sleepy(1);
        assert!(probe_status(&mut t, Duration::MAX).unwrap());
    }

    #[test]
    fn chunking_never_empty_and_zero_mtu_is_320() {
        assert_eq!(chunk_len(0), 320);
        assert_eq!(chunk_len(127), 127);
        assert_eq!(chunk_len(1_000_000), 65535);
        let data = vec![7u8; 1000];
        let sizes: Vec<usize> = mtu_chunks(&data, 0).map(<[u8]>::len).collect();
        assert_eq!(sizes, [320, 320, 320, 40]);
        for mtu in [0, 1, 2, 31, 320, 999, 1000, 1001, 70_000] {
            let chunks: Vec<&[u8]> = mtu_chunks(&data, mtu).collect();
            assert!(chunks.iter().all(|c| !c.is_empty()));
            assert_eq!(chunks.concat(), data);
        }
        assert_eq!(mtu_chunks(&[], 320).count(), 0);
    }

    #[test]
    fn candidate_order() {
        assert_eq!(channel_candidates(None, None), [1, 2]);
        assert_eq!(channel_candidates(Some(1), Some(1)), [1, 2]);
        assert_eq!(channel_candidates(Some(3), Some(2)), [3, 2, 1]);
        assert_eq!(channel_candidates(None, Some(2)), [2, 1]);
        assert_eq!(channel_candidates(Some(2), None), [2, 1]);
        assert_eq!(channel_candidates(Some(0), Some(31)), [1, 2]);
    }

    #[test]
    fn printer_cod() {
        assert!(is_printer_class_of_device(0x04_2680));
        assert!(!is_printer_class_of_device(0x24_0404)); // audio headset
        assert!(!is_printer_class_of_device(0x00_0600)); // imaging, no printer bit
    }

    #[test]
    fn frame_detection() {
        let mut f = vec![0u8; 32];
        f[..3].copy_from_slice(&[0x80, 0x20, 0x42]);
        assert!(contains_status_frame(&f));
        assert!(!contains_status_frame(&f[..31]));
        let mut g = vec![0x80, 0x20];
        g.extend_from_slice(&f);
        assert!(contains_status_frame(&g));
    }
}
