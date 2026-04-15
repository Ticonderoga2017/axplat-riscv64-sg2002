use axplat::{
    console::ConsoleIf,
    mem::{pa, phys_to_virt},
};
use kspin::SpinNoIrq;
use lazyinit::LazyInit;
use uart_16550::MmioSerialPort;

use crate::{
    config::devices::UART_PADDR,
    irq::{UART_LSR_VA, UART_USR_VA, uart_enable_erbfi},
};

static UART: LazyInit<SpinNoIrq<MmioSerialPort>> = LazyInit::new();

pub(crate) fn init_early() {
    UART.init_once({
        let uart =
            unsafe { MmioSerialPort::new_with_stride(phys_to_virt(pa!(UART_PADDR)).as_usize(), 4) };
        // uart.init();
        SpinNoIrq::new(uart)
    });
    // 清理 DW-APB-UART 残留中断状态
    unsafe {
        core::ptr::read_volatile(UART_USR_VA as *const u32); // 读 USR 
        core::ptr::read_volatile(UART_LSR_VA as *const u32); // 读 IIR
    }
    // 使能 ERBFI，使得后续 PLIC enable IRQ 44 后 UART 能产生 RX 中断
    uart_enable_erbfi();
}

struct ConsoleIfImpl;

#[impl_plat_interface]
impl ConsoleIf for ConsoleIfImpl {
    /// Writes bytes to the console from input u8 slice.
    fn write_bytes(bytes: &[u8]) {
        for &c in bytes {
            let mut uart = UART.lock();
            match c {
                b'\n' => {
                    uart.send_raw(b'\r');
                    uart.send_raw(b'\n');
                }
                c => uart.send_raw(c),
            }
        }
    }

    /// Reads bytes from the console into the given mutable slice.
    /// Returns the number of bytes read.
    fn read_bytes(bytes: &mut [u8]) -> usize {
        let mut uart = UART.lock();
        for (i, byte) in bytes.iter_mut().enumerate() {
            match uart.try_receive() {
                Ok(c) => *byte = c,
                Err(_) => {
                    // 读完所有数据后，重新使能 ERBFI
                    drop(uart);
                    uart_enable_erbfi();
                    return i;
                }
            }
        }
        drop(uart);
        uart_enable_erbfi();
        bytes.len()
    }

    /// Returns the IRQ number for the console, if applicable.
    #[cfg(feature = "irq")]
    fn irq_num() -> Option<usize> {
        Some(crate::config::devices::UART_IRQ)
    }
}
