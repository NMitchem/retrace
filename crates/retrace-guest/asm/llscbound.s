// M42 final-review fix (F1): the witness for run()'s 16-step pair prologue (spec §3e, plan R10).
// A separate fixture, so llsc.s and its window coordinates stay frozen.
//
// A load-exclusive whose sequence a branch leaves: the cell is nonzero, so the cbnz is taken and no
// store-exclusive follows. Stepped past the ldxr, the shadow is set, and it stays set through every
// instruction below, since none of them is a store-exclusive, a clrex or an exit. run() entered
// there steps PAIR_STEP_BOUND (16) instructions, still inside `far`, then must drop the shadow
// before it resumes natively.
//
// The invariant the test depends on:
//   - bnd_bp is reached only after more than 16 steps from the cbnz (1 + 20 adds), so the prologue
//     cannot step onto it: the stop at bnd_bp is the native loop's hardware breakpoint;
//   - bnd_bp is more than 16 words after bnd_ldx, so §3d's 16-word backward scan from it cannot
//     reach the load, and the native stop infers nothing. A shadow there is a stale one.
// Nothing between bnd_ldx and bnd_bp exits, so no non-debug exit clears the shadow first.
//
// One window: the exit. Replay's divergence oracle compares exit's x0..x7, so x3, which every add
// bumps, is checked there too.
.section __TEXT,__text
.global _start
.p2align 2
_start:
    adrp x9, cellbnd@PAGE
    add  x9, x9, cellbnd@PAGEOFF
bnd_ldx:
    ldxr w10, [x9]                  // K = 2 in window 1; the cell holds 1
bnd_cbnz:
    cbnz w10, far                   // taken: this load-exclusive has no store-exclusive
bnd_stx:
    stxr wzr, w10, [x9]             // never executed
far:
    add  x3, x3, #1                 // 20 non-exiting instructions: the prologue's 16 steps end here
    add  x3, x3, #1
    add  x3, x3, #1
    add  x3, x3, #1
    add  x3, x3, #1
    add  x3, x3, #1
    add  x3, x3, #1
    add  x3, x3, #1
    add  x3, x3, #1
    add  x3, x3, #1
    add  x3, x3, #1
    add  x3, x3, #1
    add  x3, x3, #1
    add  x3, x3, #1
    add  x3, x3, #1
    add  x3, x3, #1
    add  x3, x3, #1
    add  x3, x3, #1
    add  x3, x3, #1
    add  x3, x3, #1
bnd_bp:
    add  x3, x3, #1                 // 23 words after bnd_ldx; 21 steps after bnd_cbnz
    add  x3, x3, #1
    add  x3, x3, #1
    add  x3, x3, #1
    mov  x0, #0
    mov  x16, #1                    // SYS_exit(0)
bnd_svc:
    svc  #0x80

.section __DATA,__data
.p2align 6
cellbnd: .word 1                    // NONZERO: the cbnz is taken
