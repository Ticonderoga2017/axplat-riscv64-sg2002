<<<<<<< HEAD
# ArceOS / Starry OS 移植 sg2002

## 参考文档

- [昉·星光 2](https://doc.rvspace.org/Doc_Center/visionfive_2.html)
- [昉·星光 2单板计算机快速参考手册](https://doc.rvspace.org/VisionFive2/Quick_Start_Guide/index.html)
- [昉·星光 2单板计算机软件技术参考手册](https://doc.rvspace.org/VisionFive2/SW_TRM/index.html)
- [昉·惊鸿-7110启动手册](https://doc.rvspace.org/VisionFive2/Developing_and_Porting_Guide/JH7110_Boot_UG/index.html)

## 快速开始

### 1. 编译 Starry OS

```bash
$ cd StarryOS
# 正常编译
$ make vf2
# 推荐开启 link time optimizations
$ make vf2 LTO=y
# 调试日志输出
$ make vf2 LOG=debug
```

内核镜像文件位于 `StarryOS_visionfive2.bin`

### 2. 准备 SD 卡

TODO: 初始化分区表、OpenSBI 等

### 3. 准备启动分区

1. 创建一个 FAT32 格式的分区
2. 将编译好的内核镜像文件拷贝至根目录并重命名为 `kernel`
3. 创建文件 `vf2_uEnv.txt`，写入以下内容：
   ```
   boot2=load mmc 1:3 $kernel_addr_r kernel; go $kernel_addr_r
   ```
   其中 `1:3` 代表 1 号卡槽的分区 3，请根据实际情况调整；环境变量中 `kernel_addr_r` 应为 `0x40200000`，如果不是的话请在此文件中进行覆盖

到这里，应当可以成功进入 ArceOS 并打印调试信息

### 4. 准备文件系统

1. 创建一个 ext4 格式的分区，并将“分区名称”设置为 `root`（注意不是卷标）；这里假设创建的分区是 `/dev/sda4`
2. 将 rootfs 刷写到此分区，如：
   ```bash
   sudo dd if=rootfs-riscv64.img of=/dev/sda4 status=progress bs=4M conv=fsync
   ```
   推荐先多次使用 `resize2fs -M xxx.img` 尽可能压缩镜像文件大小以加快刷写速度
3. 更新文件系统大小，扩大到整个分区：
   ```bash
   sudo resize2fs /dev/sda4
   ```

至此，应当可以进入 Starry OS 的命令行进行交互，不过由于目前还未实现网卡驱动，所以无法使用 apk 安装软件包，可以在创建基础文件系统后，将需要运行的软件拷贝至文件系统。

## TODO

- PLIC 无法工作

## 移植说明

该平台与 QEMU RISC-V Virt Machine 相似程度较高，本仓库的适配代码与 `axplat-riscv64-qemu-virt` 也仅存在一些配置上的差异。以下对其简单说明：

1. 配置文件：参考 [Linux 中的设备树配置文件](https://github.com/torvalds/linux/blob/master/arch/riscv/boot/dts/starfive/jh7110-common.dtsi)，修改 axconfig.toml，主要有以下内容需要调整：
   - `phys-memory-base`/`phys-memory-size`：物理内存区域
   - `kernel-base-paddr`/`kernel-base-vaddr`：内核代码基地址
   - `mmio-ranges`：这里我们为了方便直接把整个 `0x0` 到 `0x4000_0000` 都配置成了 MMIO 区域
   - `pci-*`：目前没有实现 PCI
   - `timer-frequency`：时钟频率
   - `rtc-paddr`/`plic-paddr`/`uart-paddr`/`uart-uirq`/`sdmmc-paddr`：外设相关配置

   `timer-irq` 和 `ipi-irq` 在 RISC-V 架构上是固定的。

2. 启动：最初我们在 U-Boot 中使用 booti 指令启动，因此伪装了 [Linux 启动镜像文件头](https://www.kernel.org/doc/html/v5.8/riscv/boot-image-header.html)，即代码 `boot.rs` 中 `.ascii  \"MZ\"` 这一段。后来我们才发现可以直接用 `go` 指令更方便地直接进行跳转，不过这一段文件头因为可以兼容两种启动方式就保留了下来。
3. CPU 配置：VIsionFive 2 所使用的 JH7110 处理器有四个 64 位 RISC-V CPU（支持 rv64gc，编号为1-4）和一个 32 位 RISC-V CPU（支持rv32imfc，编号为0），因此 U-Boot 不会在 0 号核上启动，ArceOS 也无法在它上面运行。然而 ArceOS 许多设计都假定 cpuid 从 0 开始，我们把 cpu id 从原始的 1-4 映射到 0-3 来解决这个问题。
4. 存储设备驱动：[Simple SD/MMC Driver](https://github.com/Starry-OS/simple-sdmmc)
=======
# ArceOS / Starry OS 移植 SG2002  

本仓库是 [ArceOS](https://github.com/arceos-org/arceos) / [Starry OS](https://github.com/Starry-OS/StarryOS) 在算能 (Sophgo) SG2002 平台上的适配层（platform crate）。  

## 参考文档  

- [SG2002 技术参考手册 (TRM v1.02)](https://github.com/Ticonderoga2017/sophgo-doc-sg2002-trm-v1.02)  
- [算能 SG200x 开发者文档](https://developer.sophgo.com/thread/556.html)  
- [Linux 设备树 - Sophgo CV18xx/SG200x](https://github.com/torvalds/linux/tree/master/arch/riscv/boot/dts/sophgo)  
  
## 硬件概述  

SG2002 处理器包含：  

- 1x C906 大核（RISC-V 64-bit，rv64gc，1GHz）— 主核，运行 ArceOS/Starry OS  
- 1x C906 小核（RISC-V 64-bit，700MHz）  
- 1x 8051 MCU  
- 256MB DDR（实际可用约 254MB）  
  

当前配置仅使用大核（`cpu-num = 1`）。  

## 快速开始  

### 1. 编译 Starry OS  

```bash  
$ cd StarryOS  
# 正常编译  
$ make sg2002  
# 推荐开启 link time optimizations  
$ make sg2002 LTO=y  
```

编译目标默认开启 `LOG=debug`。内核镜像文件位于 `StarryOS_sg2002.elf`（二进制文件为 `StarryOS_sg2002.bin`）。  

### 2. 准备 SD 卡  

SG2002 开发板使用算能提供的引导流程（`fip.bin` 包含 OpenSBI + U-Boot）。请参考开发板文档完成以下步骤：  

1. 对 SD 卡进行分区，确保包含引导分区（FAT32）和 `fip.bin` 等引导文件  
2. 具体的分区布局和引导文件烧写方式请参考所使用开发板（如 LicheeRV Nano、MilkV Duo 等）的官方文档  

### 3. 准备启动分区  

1. 确保 SD 卡上有一个 FAT32 格式的分区  
2. 将编译好的内核镜像文件拷贝至根目录并重命名为 `kernel`  
3. 在 U-Boot 命令行中执行以下命令加载并启动内核：  
   ```  
   load mmc 0:1 0x80200000 kernel; go 0x80200000  
   ```
   其中 `0:1` 代表 0 号卡槽的分区 1，请根据实际情况调整；加载地址 `0x80200000` 对应 `axconfig.toml` 中的 `kernel-base-paddr`。  
  
   如需自动启动，可将上述命令写入 U-Boot 的 `boot.scr` 或环境变量中。  

到这里，应当可以成功进入 ArceOS 并打印调试信息。  

### 4. 准备文件系统  

1. 创建一个 ext4 格式的分区，并将"分区名称"设置为 `root`（注意不是卷标）；这里假设创建的分区是 `/dev/sda4`  
2. 将 rootfs 刷写到此分区，如：  
   ```bash  
   sudo dd if=rootfs-riscv64.img of=/dev/sda4 status=progress bs=4M conv=fsync  
   ```
   推荐先多次使用 `resize2fs -M xxx.img` 尽可能压缩镜像文件大小以加快刷写速度  
3. 更新文件系统大小，扩大到整个分区：  
   ```bash  
   sudo resize2fs /dev/sda4  
   ```

至此，应当可以进入 Starry OS 的命令行进行交互。目前已初步支持 AIC8800 WiFi SDIO 驱动（通过 `aic8800_sdio`），但网络功能可能尚不完善，建议在创建基础文件系统时将需要运行的软件预先拷贝至文件系统。  

## 已知问题  

- PLIC 存在硬件 quirk：enable 区的读操作会触发 LoadFault，已通过 shadow 寄存器方式绕过（详见 `src/irq.rs`）  
- `boot.rs` 中 `_start` 的注释 `// PC = 0x4020_0000` 可能有误，实际内核加载地址应为 `0x8020_0000`（以 `axconfig.toml` 中 `kernel-base-paddr` 为准）  
- UART IRQ 当前未启用（`console.rs` 中 `irq_num()` 返回 `None`）  
  
## 移植说明  

该平台与 QEMU RISC-V Virt Machine 相似程度较高，本仓库的适配代码与 `axplat-riscv64-qemu-virt` 也仅存在一些配置上的差异。以下对其简单说明：  

### 1. 配置文件  

参考 [Linux 中的设备树配置文件](https://github.com/torvalds/linux/tree/master/arch/riscv/boot/dts/sophgo)，修改 `axconfig.toml`，主要有以下内容需要调整：  

- `phys-memory-base`/`phys-memory-size`：物理内存区域（`0x8000_0000`，254MB）  
- `kernel-base-paddr`/`kernel-base-vaddr`：内核代码基地址（`0x8020_0000`）  
- `mmio-ranges`：这里我们为了方便直接把整个 `0x0` 到 `0x8000_0000` 都配置成了 MMIO 区域  
- `timer-frequency`：时钟频率（4MHz）  
- `plic-paddr`：PLIC 基地址（`0x7000_0000`）  
- `rtc-paddr`：RTC 基地址（`0x0502_6000`）  
- `uart-paddr`/`uart-irq`：UART 相关配置（`0x0414_0000`，IRQ `0x2c`）  
- `sdmmc-paddr`：SD/MMC 控制器基地址（`0x0431_0000`）  
- `sdio1-paddr`/`sdio1-irq`：WiFi SDIO1 控制器（AIC8800，`0x0432_0000`，IRQ 38）  
  

`timer-irq` 和 `ipi-irq` 在 RISC-V 架构上是固定的。  

### 2. 启动  

通过 U-Boot 的 `go` 指令直接跳转到内核入口地址 `0x80200000`。`boot.rs` 中的 `_start` 函数完成以下工作：  

1. 保存 hartid 和 DTB 指针  
2. 设置启动栈  
3. 初始化 Sv39 启动页表（恒等映射 + 高地址线性映射）  
4. 通过 UART 直接写寄存器输出 `Boot` 字符串（早期调试）  
5. 开启 MMU  
6. 跳转到高地址空间的 `call_main`  

### 3. CPU 配置  

SG2002 的 C906 大核 hartid 为 1（而非 0），但 ArceOS 许多设计假定 cpuid 从 0 开始。因此在 `boot.rs` 中通过 `addi a0, a0, -1` 将 hartid 从 1 映射到 0。  

### 4. PLIC 中断控制器  

SG2002 的 PLIC（`0x7000_0000`）存在硬件问题：enable 区（`0x7000_2100+`）的读操作会触发 LoadFault。为此 `src/irq.rs` 中实现了以下 workaround：  

- **冷初始化**：仅写 priority 区，不读 enable 区（`plic_cold_init_once`）  
- **Shadow enable 寄存器**：在内存中维护 enable 位的副本，仅执行写操作（`plic_enable_disable_raw`）  
- **Claim/Complete**：使用内核虚拟地址（`PLIC_KERNEL_VA`）直接操作，避免 `riscv_plic` 库内部 32 位基址截断导致的地址错误  
  
### 5. 串口驱动  

使用 `uart_16550` crate 驱动 Synopsys DesignWare APB UART（`snps,dw-apb-uart`），寄存器步进为 4 字节（`reg-shift = 2`）。  

### 6. 存储设备驱动  

SD 卡控制器使用 [sdhci-cv1800](https://github.com/Ticonderoga2017/StarryOS/tree/sg2002/wireless/sdhci-cv1800) 驱动，兼容 CV181x 系列 SD 控制器。  

### 7. WiFi 驱动  

通过 SDIO1 控制器（`0x0432_0000`）连接 AIC8800 WiFi 模块，相关驱动：  

- [aic8800_sdio](https://github.com/Ticonderoga2017/StarryOS/tree/sg2002/wireless/aic8800_sdio) — SDIO 传输层  
- [aic8800_fw](https://github.com/Ticonderoga2017/StarryOS/tree/sg2002/wireless/aic8800_fw) — 固件加载  
- [aic8800_fdrv](https://github.com/Ticonderoga2017/StarryOS/tree/sg2002/wireless/aic8800_fdrv) — 功能驱动  
- [aic8800-sdio-firmware](https://github.com/Ticonderoga2017/aic8800-sdio-firmware) — 固件文件  
  
## 源码结构  

```  
src/  
├── boot.rs      — 启动代码（页表初始化、MMU 开启、入口跳转）  
├── console.rs   — 串口驱动（UART 16550 MMIO）  
├── init.rs      — 平台初始化  
├── irq.rs       — 中断处理（PLIC workaround、timer/IPI/外部中断）  
├── lib.rs       — crate 入口与配置加载  
├── mem.rs       — 内存管理  
├── power.rs     — 电源管理（关机/重启）  
└── time.rs      — 时钟与定时器  
```
>>>>>>> 2d238a3 (扩展config地址字段)
