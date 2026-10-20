# Local patches to ptouch-transport

Vendored from <https://github.com/fjanicki/ptouch> at rev
`485dab45d98083a9124958e531a8f39aede14ecd`, directory `crates/ptouch-transport` (sources, tests,
licences). The root `Cargo.toml` of baradar points `[patch."https://github.com/fjanicki/ptouch"]`
at this directory, so every crate that depends on the git `ptouch-transport` gets this copy.

## Cargo.toml

Upstream inherits package keys, dependency versions and lints from its workspace. They are
written out here with the values of upstream's workspace at that rev (same exact `=` pins).
`ptouch` points at the same git URL and rev as baradar's own dependency, so both resolve to one
package and `ptouch` types unify. `readme` is dropped (upstream's README is not vendored);
`publish = false` is added. A `[[test]]` entry registers `tests/host_run_loop.rs` (below).

## Test fixture

`tests/virtual_transport.rs` and `tests/tcp.rs` include a status fixture from the sibling
`ptouch` crate (`../../ptouch/tests/fixtures/status/p710bt_24mm_laminated_idle.hex`). That file is
copied to `tests/fixtures/` and both `include_str!` paths point there.

## Patch 1: macOS host run loop mode

### Problem

The macOS backend sends every IOBluetooth call through `macos/runtime.rs` `submit()` to the
`RUNTIME` channel. Only `run_main_loop` (via `run_with_event_loop`) consumes that channel, by
turning the main thread into a blocking job/`CFRunLoop` pump loop. In a GUI application (GPUI,
AppKit) the main thread runs `NSApplication`'s run loop and can never enter that loop, so every
Bluetooth operation failed with `EventLoopNotRunning`.

### Change

New public API in `ptouch_transport::macos`:

- `use_host_run_loop()`: switches on host run loop mode (once, at startup; irreversible).
- `host_run_loop_enabled() -> bool`.
- `shutdown_host_run_loop()`: what `run_main_loop` does when its closure returns (close open
  channels and inquiries, release basebands, leak delegates), for host mode.
- `run_on_main_thread(f) -> Result<R, TransportError>`: runs a closure on the main thread
  through whichever loop is active and waits. It exposes the existing `runtime::call` for
  IOBluetooth work the crate does not wrap (baradar uses it for `IOBluetoothDevicePair`), so
  that work is serialised with the backend's own jobs; it is also what the test uses.

`macos/runtime.rs`:

- `submit()`: if the private loop runs, it still gets the job (unchanged). Otherwise, in host
  mode, the job goes to `DispatchQueue::main().exec_async` and runs against a main-thread-local
  `BtState` (`HOST_STATE`). Called on the main thread it still returns `EventLoopNotRunning`,
  since waiting there would deadlock. If a job is ever entered re-entrantly (a nested run loop
  draining the main queue while a job runs), it is re-queued instead of panicking on the
  `RefCell`.
- `is_running()`: also `true` in host mode. So `run_main_loop` (and with it
  `run_with_event_loop`) just calls `f()`, and `paired_devices()` routes through the main thread
  when called from a worker.
- `shutdown()` is split into `begin_shutdown` (stop inquiries, `closeChannel`) and
  `finish_shutdown` (clear delegates, `closeConnection`, leak delegates), with the same order and
  drains as before for the private loop. `host_shutdown()` runs both as main-queue jobs from a
  worker (drains are sleeps on the worker while the host pumps), or directly without draining
  on the main thread.
- `pump`/`pump_for` are only used by the private loop and its shutdown. In host mode the host's
  run loop pumps and nothing blocks the main thread.

Places checked that needed no change:

- `rfcomm.rs` `CLOSE_DRAIN`, `close_wait`, SDP polling and `NOT_OPEN_RETRY` sleeps run on the
  calling (worker) thread, so the host keeps pumping meanwhile.
- `rfcomm.rs` `close_inner` treats `EventLoopNotRunning` as "already shut down". That only
  happens in host mode when closing from the main thread (not supported, documented).
- `serial.rs` `CLOSE_DRAIN` is a serial-port drain on the calling thread (unrelated).

Doc updates: the module docs of `macos`, `runtime`, `runloop`, `lib.rs`, `endpoint::open`, and
the `TransportError::EventLoopNotRunning` doc and message now mention host mode.

### Caveat (unchanged behaviour, more visible in host mode)

If a channel never delivers `rfcommChannelWriteComplete:`, the backend falls back to `writeSync`
(PROTOCOL.md §2.2 step 7), which blocks the main thread for each 4 KiB batch. In host mode that
means the UI thread. Upstream already bounds the worker's wait (`write_timeout`). Whether macOS
26/27 takes that path is still unverified on hardware.

### Verification

`tests/host_run_loop.rs` (`harness = false`, so `main` is the process's main thread) calls no
IOBluetooth API. It checks that:

- worker submissions are refused before host mode, and refused on the main thread in host mode;
- `run_with_event_loop` just calls its closure in host mode;
- 4 workers × 25 `run_on_main_thread` jobs, submitted concurrently while the main thread runs
  `CFRunLoopRunInMode` slices (like `NSApplication`), all run on the main thread, in order;
- `shutdown_host_run_loop` returns from a worker, and returns at once on the main thread.

Run it from the workspace root with `cargo test -p ptouch-transport --test host_run_loop` (the
vendored crate is not a workspace member, so `--features` cannot be passed; the defaults include
`macos-rfcomm`). The whole vendored suite passes the same way: `cargo test -p ptouch-transport`.

### Upstreaming

The patch is self-contained (`src/`: 6 files, +197/−44 lines, about half of it docs) and does
not change behaviour when `use_host_run_loop` is never called. It could go upstream as is.

## Patch 2: no paging on the main thread

### Problem

With Patch 1, every IOBluetooth job runs on the host's main thread. Three of them were
synchronous calls that page the device: `openConnection:withPageTimeout:authenticationRequired:`
with a nil target (step 3), `openRFCOMMChannelSync` (step 5) and the multiplexer prime's
`openRFCOMMChannelSync` on channel 1 (step 10). While a printer is off or out of range, each one
blocks the main thread for a page timeout or longer. Baradar auto-reconnects to the last printer
at launch and retries the open three times, so the whole UI froze for about a minute.

### Change

`macos/rfcomm.rs` (+ `ConnectionTarget` in `macos/delegates.rs`):

- `open_baseband` passes a `ConnectionTarget`, which makes the open asynchronous, and the worker
  waits (`wait_baseband`, `PAGE_WAIT` = 10 s) for `connectionComplete:status:` while also
  polling `isConnected` every 200 ms, in case the callback never fires (like
  `sdpQueryComplete:`). Statuses map as before: success → opened, `kIOReturnTimeout` or no
  connection by the deadline → fail fast, anything else → let the RFCOMM open page.
- `open_channel_job` makes the sync attempt only when the baseband is already connected (the
  usual case after `open_baseband`, and the path verified on hardware); otherwise it goes
  straight to the async open, which `attempt` already waits for on the worker.
- `prime_multiplexer` likewise primes with a sync open only over a connected baseband;
  otherwise it opens channel 1 through `attempt` (async) and closes it.

The remaining main-thread work is short getters/closes, and the `writeSync` fallback of the
caveat above.

### Verification

Builds and the vendored suite passes. Tested on a PT-E720BT with the bundled app (2026-10-20):
with the printer off, Baradar's UI stays responsive at launch while the auto-reconnect fails;
with the printer on, it connects as before. Still unknown: whether `connectionComplete:status:`
fires on macOS 26/27 (the `isConnected` poll covers it either way).
