use core::{
    num::NonZeroU32,
    ptr::NonNull,
    sync::atomic::{AtomicBool, AtomicPtr, Ordering},
};

use axplat::{
    irq::{HandlerTable, IpiTarget, IrqHandler, IrqIf},
    percpu::this_cpu_id,
};
use kspin::SpinNoIrq;
use riscv::register::sie;
use riscv_plic::Plic;
use sbi_rt::HartMask;

use crate::config::plat::PHYS_VIRT_OFFSET;

const _: () = assert!(PHYS_VIRT_OFFSET != 0);

/// 内核线性映射下 PLIC 的虚拟地址（enable/claim/complete 用此 VA，避免 riscv_plic 内 32 位基址导致 LoadFault）。
const PLIC_KERNEL_VA: usize = 0xffff_ffc0_0000_0000_usize + 0x7000_0000_usize;

/// 与 OpenSBI plic_cold_irqchip_init 一致：仅写 priority 区（首访 0x0）。不读 enable 区：该 SoC 上 enable 区 (0x70002100+) 读也会 LoadFault。
fn plic_cold_init_once() {
    const PLIC_NDEV: usize = 101; // DTS riscv,ndev = <101>
    let base = PLIC_KERNEL_VA as *mut u32;
    for i in 1..=PLIC_NDEV {
        unsafe { core::ptr::write_volatile(base.add(i), 1) };
    }
}

/// `Interrupt` bit in `scause`
pub(super) const INTC_IRQ_BASE: usize = 1 << (usize::BITS - 1);

/// Supervisor software interrupt in `scause`
#[allow(unused)]
pub(super) const S_SOFT: usize = INTC_IRQ_BASE + 1;

/// Supervisor timer interrupt in `scause`
pub(super) const S_TIMER: usize = INTC_IRQ_BASE + 5;

/// Supervisor external interrupt in `scause`
pub(super) const S_EXT: usize = INTC_IRQ_BASE + 9;

static TIMER_HANDLER: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());

static IPI_HANDLER: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());

/// The maximum number of IRQs.
pub const MAX_IRQ_COUNT: usize = 1024;

static IRQ_HANDLER_TABLE: HandlerTable<MAX_IRQ_COUNT> = HandlerTable::new();

static PLIC: SpinNoIrq<Plic> = SpinNoIrq::new(unsafe {
    Plic::new(NonNull::new(PLIC_KERNEL_VA as *mut _).unwrap())
});

fn this_context() -> usize {
    let hart_id = this_cpu_id() + 1;
    // hart 0 missing S-mode
    hart_id * 2 // supervisor context
}

/// Context 2 (S-mode) enable 区 shadow：不读 PLIC enable（该 SoC 读会 LoadFault），只写。
/// 布局：base + 0x2000 + context*0x80，每字 32 source；4 字覆盖 128 source（DTS ndev=101）。
const PLIC_CTX2_ENABLE_OFFSET: usize = 0x2000 + 2 * 0x80; // 0x2100
const PLIC_ENABLE_WORDS: usize = 4;
static ENABLE_SHADOW: SpinNoIrq<[u32; PLIC_ENABLE_WORDS]> = SpinNoIrq::new([0; PLIC_ENABLE_WORDS]);

/// 用 PLIC_KERNEL_VA 写 enable 区，避免 riscv_plic 内部 32 位基址导致 VA 0x70002104 LoadFault。
fn plic_enable_disable_raw(irq: u32, enabled: bool) {
    let word_idx = (irq as usize) / 32;
    let bit = (irq as usize) % 32;
    if word_idx >= PLIC_ENABLE_WORDS {
        return;
    }
    let mut shadow = ENABLE_SHADOW.lock();
    if enabled {
        shadow[word_idx] |= 1 << bit;
    } else {
        shadow[word_idx] &= !(1 << bit);
    }
    let val = shadow[word_idx];
    drop(shadow);
    let ptr = (PLIC_KERNEL_VA + PLIC_CTX2_ENABLE_OFFSET + word_idx * 4) as *mut u32;
    unsafe { core::ptr::write_volatile(ptr, val) };
}

/// Context 2 claim/complete 偏移：PLIC 规范 base + 0x200000 + context*0x1000。
const PLIC_CTX2_CLAIM_COMPLETE_OFFSET: usize = 0x200000 + 2 * 0x1000; // 0x202000

/// 用 PLIC_KERNEL_VA 读 claim，避免 riscv_plic 截断基址在首次外设中断时 LoadFault。
fn plic_claim_raw() -> Option<NonZeroU32> {
    let ptr = (PLIC_KERNEL_VA + PLIC_CTX2_CLAIM_COMPLETE_OFFSET) as *const u32;
    let id = unsafe { core::ptr::read_volatile(ptr) };
    NonZeroU32::new(id)
}

/// 用 PLIC_KERNEL_VA 写 complete。
fn plic_complete_raw(irq: NonZeroU32) {
    let ptr = (PLIC_KERNEL_VA + PLIC_CTX2_CLAIM_COMPLETE_OFFSET) as *mut u32;
    unsafe { core::ptr::write_volatile(ptr, irq.get()) };
}

static PLIC_COLD_INIT_DONE: AtomicBool = AtomicBool::new(false);

pub(super) fn init_percpu() {
    // 与 LicheeRV/OpenSBI 一致：先做一次 PLIC 冷初始化（priority + enable 区首访），再 per-context init
    if PLIC_COLD_INIT_DONE
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        plic_cold_init_once();
    }
    // enable soft interrupts, timer interrupts, and external interrupts
    unsafe {
        sie::set_ssoft();
        sie::set_stimer();
        sie::set_sext();
    }
    PLIC.lock().init_by_context(this_context());
}

macro_rules! with_cause {
    ($cause: expr, @S_TIMER => $timer_op: expr, @S_SOFT => $ipi_op: expr, @S_EXT => $ext_op: expr, @EX_IRQ => $plic_op: expr $(,)?) => {
        match $cause {
            S_TIMER => $timer_op,
            S_SOFT => $ipi_op,
            S_EXT => $ext_op,
            other => {
                if other & INTC_IRQ_BASE == 0 {
                    // Device-side interrupts read from PLIC
                    $plic_op
                } else {
                    // Other CPU-side interrupts
                    panic!("Unknown IRQ cause: {}", other);
                }
            }
        }
    };
}

struct IrqIfImpl;

#[impl_plat_interface]
impl IrqIf for IrqIfImpl {
    /// Enables or disables the given IRQ.
    fn set_enable(irq: usize, enabled: bool) {
        with_cause!(
            irq,
            @S_TIMER => {
                unsafe {
                    if enabled {
                        sie::set_stimer();
                    } else {
                        sie::clear_stimer();
                    }
                }
            },
            @S_SOFT => {},
            @S_EXT => {},
            @EX_IRQ => {
                let Some(irq) = NonZeroU32::new(irq as _) else {
                    return;
                };
                // priority 仍用 riscv_plic；enable/disable 用内核 VA 裸写，避免库内 32 位基址导致 0x70002104 LoadFault
                let mut plic = PLIC.lock();
                if enabled {
                    plic.set_priority(irq, 6);
                }
                drop(plic);
                plic_enable_disable_raw(irq.get(), enabled);
            }
        );
    }

    /// Registers an IRQ handler for the given IRQ.
    ///
    /// It also enables the IRQ if the registration succeeds. It returns `false` if
    /// the registration failed.
    ///
    /// The `irq` parameter has the following semantics
    /// 1. If its highest bit is 1, it means it is an interrupt on the CPU side. Its
    /// value comes from `scause`, where [`S_SOFT`] represents software interrupt
    /// and [`S_TIMER`] represents timer interrupt. If its value is [`S_EXT`], it
    /// means it is an external interrupt, and the real IRQ number needs to
    /// be obtained from PLIC.
    /// 2. If its highest bit is 0, it means it is an interrupt on the device side,
    /// and its value is equal to the IRQ number provided by PLIC.
    fn register(irq: usize, handler: IrqHandler) -> bool {
        with_cause!(
            irq,
            @S_TIMER => TIMER_HANDLER.compare_exchange(core::ptr::null_mut(), handler as *mut _, Ordering::AcqRel, Ordering::Acquire).is_ok(),
            @S_SOFT => IPI_HANDLER.compare_exchange(core::ptr::null_mut(), handler as *mut _, Ordering::AcqRel, Ordering::Acquire).is_ok(),
            @S_EXT => {
                warn!("External IRQ should be got from PLIC, not scause");
                false
            },
            @EX_IRQ => {
                if IRQ_HANDLER_TABLE.register_handler(irq, handler) {
                    Self::set_enable(irq, true);
                    true
                } else {
                    warn!("register handler for External IRQ {irq} failed");
                    false
                }
            }
        )
    }

    /// Unregisters the IRQ handler for the given IRQ.
    ///
    /// It also disables the IRQ if the unregistration succeeds. It returns the
    /// existing handler if it is registered, `None` otherwise.
    fn unregister(irq: usize) -> Option<IrqHandler> {
        with_cause!(
            irq,
            @S_TIMER => {
                let handler = TIMER_HANDLER.swap(core::ptr::null_mut(), Ordering::AcqRel);
                if !handler.is_null() {
                    Some(unsafe { core::mem::transmute::<*mut (), IrqHandler>(handler) })
                } else {
                    None
                }
            },
            @S_SOFT => {
                let handler = IPI_HANDLER.swap(core::ptr::null_mut(), Ordering::AcqRel);
                if !handler.is_null() {
                    Some(unsafe { core::mem::transmute::<*mut (), IrqHandler>(handler) })
                } else {
                    None
                }
            },
            @S_EXT => {
                warn!("External IRQ should be got from PLIC, not scause");
                None
            },
            @EX_IRQ => IRQ_HANDLER_TABLE.unregister_handler(irq).inspect(|_| Self::set_enable(irq, false))
        )
    }

    /// Handles the IRQ.
    ///
    /// It is called by the common interrupt handler. It should look up in the
    /// IRQ handler table and calls the corresponding handler. If necessary, it
    /// also acknowledges the interrupt controller after handling.
    fn handle(irq: usize) -> Option<usize> {
        with_cause!(
            irq,
            @S_TIMER => {
                trace!("IRQ: timer");
                let handler = TIMER_HANDLER.load(Ordering::Acquire);
                if !handler.is_null() {
                    // SAFETY: The handler is guaranteed to be a valid function pointer.
                    unsafe { core::mem::transmute::<*mut (), IrqHandler>(handler)() };
                }
                Some(irq)
            },
            @S_SOFT => {
                trace!("IRQ: IPI");
                let handler = IPI_HANDLER.load(Ordering::Acquire);
                if !handler.is_null() {
                    // SAFETY: The handler is guaranteed to be a valid function pointer.
                    unsafe { core::mem::transmute::<*mut (), IrqHandler>(handler)() };
                }
                Some(irq)
            },
            @S_EXT => {
                // 用内核 VA 做 claim/complete，避免 riscv_plic 截断基址在 SDIO 等外设中断时 LoadFault
                let Some(irq) = plic_claim_raw() else {
                    debug!("Spurious external IRQ");
                    return None;
                };
                trace!("IRQ: external {irq}");
                IRQ_HANDLER_TABLE.handle(irq.get() as usize);
                plic_complete_raw(irq);
                Some(irq.get() as usize)
            },
            @EX_IRQ => {
                unreachable!("Device-side IRQs should be handled by triggering the External Interrupt.");
            }
        )
    }

    /// Sends an inter-processor interrupt (IPI) to the specified target CPU or all CPUs.
    fn send_ipi(_irq_num: usize, target: IpiTarget) {
        match target {
            IpiTarget::Current { cpu_id } => {
                let res = sbi_rt::send_ipi(HartMask::from_mask_base(1 << cpu_id, 0));
                if res.is_err() {
                    warn!("send_ipi failed: {res:?}");
                }
            }
            IpiTarget::Other { cpu_id } => {
                let res = sbi_rt::send_ipi(HartMask::from_mask_base(1 << cpu_id, 0));
                if res.is_err() {
                    warn!("send_ipi failed: {res:?}");
                }
            }
            IpiTarget::AllExceptCurrent { cpu_id, cpu_num } => {
                for i in 0..cpu_num {
                    if i != cpu_id {
                        let res = sbi_rt::send_ipi(HartMask::from_mask_base(1 << i, 0));
                        if res.is_err() {
                            warn!("send_ipi_all_others failed: {res:?}");
                        }
                    }
                }
            }
        }
    }
}
