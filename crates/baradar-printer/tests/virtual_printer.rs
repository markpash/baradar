//! End-to-end print against ptouch's in-process virtual PT-E720BT (no hardware needed).

use baradar_printer::Printer;
use ptouch::Bitmap;

#[test]
fn prints_a_label_on_virtual_e720bt() -> anyhow::Result<()> {
    let mut printer = Printer::open("virtual:12")?;
    assert_eq!(printer.profile()?.name, "PT-E720BT");
    let tape = printer.tape()?;
    assert_eq!(tape.width_mm_x10, 120);
    let label = Bitmap::from_fn(120, tape.print_pins, |x, y| {
        (x / 8 + u32::from(y) / 8) % 2 == 0
    });
    printer.print_bitmap(&label)?;
    Ok(())
}
