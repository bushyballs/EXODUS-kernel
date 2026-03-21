# =============================================================================
# Hoags OS Genesis — Build System
# =============================================================================
#
# Targets:
#   make build    — Compile kernel (NASM + Cargo)
#   make run      — Build + run in QEMU
#   make iso      — Build + create bootable GRUB ISO
#   make run-iso  — Build + create ISO + boot in QEMU from ISO
#   make clean    — Remove all build artifacts
#   make easy-uefi — One-command Windows PowerShell UEFI launch path
#   make check    — Verify toolchain is installed
#
# Prerequisites:
#   rustup toolchain install nightly
#   rustup component add rust-src --toolchain nightly
#   pacman -S mingw-w64-x86_64-nasm    (MSYS2)
#   pacman -S mingw-w64-x86_64-qemu    (MSYS2, for running)
# =============================================================================

WS_TARGET  = target
KERNEL_ELF64 = $(WS_TARGET)/x86_64-unknown-none/debug/hoags-kernel
KERNEL_REL64 = $(WS_TARGET)/x86_64-unknown-none/release/hoags-kernel
KERNEL_ELF32 = $(WS_TARGET)/hoags-kernel32.elf
KERNEL_REL32 = $(WS_TARGET)/hoags-kernel32-release.elf
ISO_DIR    = $(WS_TARGET)/iso
ISO_FILE   = $(WS_TARGET)/hoags-genesis.iso
BOOTLOADER_EFI = ../bootloader-uefi/target/x86_64-unknown-uefi/release/hoags-bootloader.efi
ESP_DIR = $(WS_TARGET)/esp
OVMF_CODE ?= OVMF_CODE.fd
OVMF_VARS ?= OVMF_VARS.fd

.PHONY: build release run run-dava run-release iso run-iso uefi-build uefi-image run-uefi easy-uefi clean check

# --- Build (debug) ---
build:
	cargo +nightly build
	rust-objcopy -O elf32-i386 $(KERNEL_ELF64) $(KERNEL_ELF32)

# --- Build (release, optimized) ---
release:
	cargo +nightly build --release
	rust-objcopy -O elf32-i386 $(KERNEL_REL64) $(KERNEL_REL32)

# --- Run in QEMU (debug build) ---
run: build
	qemu-system-x86_64 \
		-kernel $(KERNEL_ELF32) \
		-serial stdio \
		-display sdl \
		-no-reboot \
		-no-shutdown \
		-m 2G \
		-smp 2 \
		-cpu max \
		-accel tcg,thread=multi

# --- Run with DAVA bridge: serial on TCP 4444 so dava_bridge.py can connect ---
run-dava: build
	qemu-system-x86_64 \
		-kernel $(KERNEL_ELF32) \
		-chardev socket,id=ser0,host=127.0.0.1,port=4444,server=on,wait=off \
		-serial chardev:ser0 \
		-display sdl \
		-no-reboot \
		-no-shutdown \
		-m 2G \
		-smp 2 \
		-cpu max \
		-accel tcg,thread=multi

# --- Run in QEMU (debug build, headless serial-only — safest on Windows) ---
run-serial-safe: build
	qemu-system-x86_64 \
		-kernel $(KERNEL_ELF32) \
		-chardev file,id=ser0,path=$(WS_TARGET)/serial.txt \
		-serial chardev:ser0 \
		-nographic \
		-no-reboot \
		-m 2G \
		-smp 2 \
		-cpu max \
		-accel tcg,thread=multi

# --- Run in QEMU (release build) ---
run-release: release
	qemu-system-x86_64 \
		-kernel $(KERNEL_REL32) \
		-serial stdio \
		-display sdl \
		-no-reboot \
		-no-shutdown \
		-m 2G \
		-smp 2 \
		-cpu max \
		-accel tcg,thread=multi

# --- Run in QEMU with no display (serial only) ---
run-serial: build
	qemu-system-x86_64 \
		-kernel $(KERNEL_ELF32) \
		-serial stdio \
		-nographic \
		-no-reboot \
		-m 512M

# --- Create bootable ISO with GRUB ---
iso: build
	mkdir -p $(ISO_DIR)/boot/grub
	cp $(KERNEL_ELF32) $(ISO_DIR)/boot/kernel.elf
	cp boot/grub.cfg $(ISO_DIR)/boot/grub/grub.cfg
	grub-mkrescue -o $(ISO_FILE) $(ISO_DIR) 2>/dev/null
	@echo ""
	@echo "ISO created: $(ISO_FILE)"
	@echo "Burn to USB: dd if=$(ISO_FILE) of=/dev/sdX bs=4M status=progress"

# --- Run ISO in QEMU ---
run-iso: iso
	qemu-system-x86_64 \
		-cdrom $(ISO_FILE) \
		-serial stdio \
		-display gtk \
		-no-reboot \
		-m 512M

# --- Build release kernel + Rust UEFI bootloader ---
uefi-build: release
	cd ../bootloader-uefi && cargo +nightly build --release --target x86_64-unknown-uefi

# --- Create UEFI ESP directory layout ---
uefi-image: uefi-build
	mkdir -p $(ESP_DIR)/EFI/BOOT $(ESP_DIR)/EFI/hoags
	cp $(BOOTLOADER_EFI) $(ESP_DIR)/EFI/BOOT/BOOTX64.EFI
	cp $(KERNEL_REL64) $(ESP_DIR)/EFI/hoags/kernel.elf

# --- Run UEFI boot path in QEMU using OVMF ---
run-uefi: uefi-image
	qemu-system-x86_64 \
		-drive if=pflash,format=raw,file=$(OVMF_CODE),readonly=on \
		-drive if=pflash,format=raw,file=$(OVMF_VARS) \
		-drive format=raw,file=fat:rw:$(ESP_DIR) \
		-serial stdio \
		-display gtk \
		-no-reboot \
		-no-shutdown \
		-m 512M

# --- Easy UEFI launch (Windows PowerShell helper) ---
easy-uefi:
	powershell -ExecutionPolicy Bypass -File tools\easy-uefi-boot.ps1

# --- Clean ---
clean:
	cargo clean
	cd ../bootloader-uefi && cargo clean
	rm -rf $(ESP_DIR)

# --- Check toolchain ---
check:
	@echo "=== Hoags OS Genesis — Toolchain Check ==="
	@echo ""
	@echo -n "Rust nightly: " && rustup run nightly rustc --version || echo "MISSING — run: rustup toolchain install nightly"
	@echo -n "rust-src:     " && (rustup component list --toolchain nightly | grep "rust-src (installed)" || echo "MISSING — run: rustup component add rust-src --toolchain nightly")
	@echo -n "NASM:         " && (nasm --version || echo "MISSING — run: pacman -S mingw-w64-x86_64-nasm")
	@echo -n "QEMU:         " && (qemu-system-x86_64 --version 2>/dev/null | head -1 || echo "MISSING — run: pacman -S mingw-w64-x86_64-qemu")
	@echo -n "LLD:          " && (rust-lld --version 2>/dev/null | head -1 || echo "(provided by rustup)")
	@echo ""
	@echo "=== Done ==="
