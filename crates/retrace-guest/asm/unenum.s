.section __TEXT,__text
.global _start
_start:
    mov  x16, #8              // the kernel's nosys slot (old creat): no syscall, so no row — ever
    svc  #0x80
    mov  x0, #0
    mov  x16, #1              // SYS_exit(0)
    svc  #0x80
