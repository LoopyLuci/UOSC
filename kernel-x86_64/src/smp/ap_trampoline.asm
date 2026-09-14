; Real x86 AP (application processor) startup trampoline.
;
; This is the actual 16-bit real-mode entry point an AP starts executing at
; after a real INIT-SIPI-SIPI sequence (see `smp.rs`): the SIPI vector
; encodes CS = (load_address >> 12), IP = 0, so the AP's very first
; instruction fetch after SIPI is the first byte of this file, and nothing
; else — no stack, no GDT, no paging, 16-bit real-mode addressing only,
; exactly the same constrained starting state the BSP itself would have if
; it were reset. This file is what takes the AP from that bare state
; through 32-bit protected mode into real 64-bit long mode and jumps into
; genuine Rust kernel code (`smp::ap_entry64`), following the same
; real-mode -> protected-mode -> long-mode path every x86-64 CPU (including
; this kernel's own BSP boot path, via the `bootloader` crate) has to take.
;
; `ORG 0x8000` makes every label in this file an *absolute* address already
; assuming the assembled bytes are loaded starting at physical 0x8000 —
; `smp.rs` copies this exact blob to that exact physical address before
; sending the SIPI, so every reference below (`lgdt [gdt_ptr]`, the far
; jumps, the data slot reads) is correct with no runtime relocation needed.
; `smp.rs::TRAMPOLINE_PHYS_ADDR` and this `ORG` must be kept in sync — see
; that module's docs.
;
; Three real values the BSP pokes into this same physical page before
; sending the SIPI (see the `ap_cr3`/`ap_stack_top`/`ap_entry_addr` labels
; below — exported so `smp.rs` can find their absolute addresses without
; hand-counting bytes, via the NASM map file `build.rs` parses):
;   - `ap_cr3`: the physical frame of the *same* L4 page table the BSP
;     itself runs on — the AP enters long mode over the kernel's one real,
;     already-built set of page tables, not a duplicate.
;   - `ap_stack_top`: a real, dedicated stack for the AP, heap-allocated by
;     the BSP (this AP does not touch the BSP's own stack).
;   - `ap_entry_addr`: the real virtual address of `smp::ap_entry64`, the
;     Rust function this trampoline hands off to once long mode is live.

[map all ap_trampoline.map]

BITS 16
ORG 0x8000

CODE32_SEL equ 0x08
DATA32_SEL equ 0x10
CODE64_SEL equ 0x18
DATA64_SEL equ 0x20

global ap_trampoline_start
ap_trampoline_start:
    cli
    xor ax, ax
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov sp, 0x7c00          ; unused in practice (no pushes before long mode), kept valid regardless

    lgdt [gdt_ptr]

    ; Enter protected mode (CR0.PE).
    mov eax, cr0
    or eax, 1
    mov cr0, eax

    ; Real mode -> 32-bit protected mode: a far jump reloads CS with a real
    ; 32-bit-flat code descriptor and flushes the real-mode instruction
    ; prefetch queue, both required — CS doesn't take effect from a plain
    ; mov, and stale real-mode-decoded bytes already fetched must not run
    ; as 32-bit code.
    jmp CODE32_SEL:pmode_entry

BITS 32
pmode_entry:
    mov ax, DATA32_SEL
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov fs, ax
    mov gs, ax

    ; PAE must be on before a long-mode-capable CR3 (pointing at real PML4
    ; entries) is loaded and before EFER.LME is set — the exact hardware
    ; ordering every x86-64 long-mode transition requires.
    mov eax, cr4
    or eax, (1 << 5)        ; CR4.PAE
    mov cr4, eax

    ; The real L4 table frame the BSP wrote into this same page — not a
    ; second, separate page table, the kernel's one real set (see module
    ; docs above and `smp.rs::start_ap`).
    mov eax, [ap_cr3]
    mov cr3, eax

    ; EFER.LME (Long Mode Enable) *and* EFER.NXE via a real RDMSR/WRMSR —
    ; MSR 0xC0000080. NXE matters here for a real, non-obvious reason: the
    ; AP walks the exact same, already-built page tables the BSP shares
    ; (see module docs), and the BSP's own boot (cpu_features.rs) already
    ; set real EFER.NXE=1 and marked its heap/stack pages NX — including
    ; the very stack this AP is about to use. Per the real Intel SDM
    ; semantics, bit 63 of a paging-structure entry is only a valid NX bit
    ; when EFER.NXE=1 in the *walking* CPU; with NXE=0 that bit is
    ; *reserved* instead, and any access through an entry with it set
    ; faults with a reserved-bit violation — a real bug this trampoline hit
    ; on its very first boot (error code 0xA — write + reserved-bit — while
    ; touching this AP's own, entirely ordinary stack), not a hypothetical
    ; one. See STATUS.md for the full real QEMU trace that diagnosed it.
    mov ecx, 0xC0000080
    rdmsr
    or eax, (1 << 8) | (1 << 11)
    wrmsr

    ; CR0.PG — this is the actual transition into long mode (CPU is now in
    ; "compatibility mode" until CS is reloaded with an L-bit code segment,
    ; the far jump immediately below).
    mov eax, cr0
    or eax, (1 << 31)
    mov cr0, eax

    jmp CODE64_SEL:lmode_entry

BITS 64
lmode_entry:
    mov ax, DATA64_SEL
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov fs, ax
    mov gs, ax

    ; A real, dedicated stack for this core — never the BSP's.
    mov rsp, [ap_stack_top]

    ; Hand off to real Rust: `smp::ap_entry64`'s real virtual address, valid
    ; now that CR3 (loaded above) covers the whole shared kernel address
    ; space, the same mapping that makes it valid for the BSP.
    mov rax, [ap_entry_addr]
    jmp rax

; --- A minimal, temporary 32/64-bit GDT — just enough to make the two far
; jumps above legal. Long-lived, real per-CPU GDT/TSS setup happens in Rust
; (`gdt::init_for_ap`) immediately after `ap_entry64` starts running; this
; one is never used again after that. ---
align 16
gdt_start:
    dq 0                                ; null
    dq 0x00CF9A000000FFFF               ; 0x08: 32-bit code, base 0, limit 4G, DPL0
    dq 0x00CF92000000FFFF               ; 0x10: 32-bit data, base 0, limit 4G, DPL0
    dq 0x00AF9A000000FFFF               ; 0x18: 64-bit code (L=1, D=0), DPL0
    dq 0x00AF92000000FFFF               ; 0x20: 64-bit data, DPL0
gdt_end:

gdt_ptr:
    dw gdt_end - gdt_start - 1
    dd gdt_start

align 8
global ap_cr3
ap_cr3: dq 0

global ap_stack_top
ap_stack_top: dq 0

global ap_entry_addr
ap_entry_addr: dq 0

global ap_trampoline_end
ap_trampoline_end:
