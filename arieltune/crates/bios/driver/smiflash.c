// SPDX-License-Identifier: GPL-2.0
//
// smiflash — BC-250 (AMI Aptio V) kernel -> SMM -> SPI-flash R/W primitive.
//
// WHAT THIS IS (responsible-use framing)
// --------------------------------------
// A firmware-research / owner-control tool for hardware you own. It lets the OS
// read and write the board's own SPI flash by invoking the platform firmware's
// OWN intended SMM update mechanism — the same SW-SMI path a vendor BIOS updater
// (AMI AFU) uses — NOT a novel exploit or a memory-corruption bug. It is
// board-specific: BC-250 / AMI Aptio V, with the SW-SMI command port read from
// the FADT SMI_CMD field.
//
// It also has a defensive purpose: understanding and testing your own board's
// SMM / SPI-flash attack surface is exactly what open firmware-security tools
// (CHIPSEC, coreboot, flashrom) exist for. Use it to audit the update path on a
// board you control.
//
// The caller (arieltune) enforces the safety gates: BC-250 detection before any
// SMI is fired, and an APCB-slot write guard so a write cannot clobber firmware
// structures it should not touch. This module itself is a dumb single-SMI firer.
//
// HOW IT WORKS
// ------------
// AMI's vendor flash interface (SmiFlash, the AFU back-end) registers a SW-SMI
// handler (module BC327DBD-…, handler RVA 0xb68) via SwDispatch2 for SW-SMI
// command values 0x20..0x25. The handler reads a 24-byte command-struct pointer
// from the interrupted CPU's save state — split across save-state registers
// 0x27 (RBX, low 32) and 0x28 (RCX, high 32) — then routes by the command byte:
//
//   0x20 begin   (acquire SPI lock)        0x21 read   (-> data buffer)
//   0x22 4K read (internal)                0x23 write  (data buffer -> flash)
//   0x24 end     (release SPI lock)        0x25 finish/cleanup
//
// Command struct (24 bytes):
//   +0x00 u64 data_phys   physical address of the data buffer
//   +0x08 u32 field       flash addressing: pass the CHIP OFFSET directly
//                         (the handler subtracts 0x1000000, which wraps the
//                          u32 to land in the 0xFF000000 BIOS MMIO window)
//   +0x0C u32 size        byte count
//   +0x10 u8  status      handler-written: 0 = OK, 1 = error
//
// THE LOAD-BEARING DETAIL (cost us the whole investigation):
//   the SW-SMI is triggered by writing to the platform SMI COMMAND PORT, which
//   on the BC-250 is **0xB0**, NOT the PC-conventional 0xB2. Get the real port
//   from the FADT SMI_CMD field — never assume 0xB2.
//   It is a module parameter here so this driver ports to other boards.
//
// USAGE
//   insmod smiflash.ko smi_port=0xB0
//   ioctl(open("/proc/smiflash"), SMIFLASH_DO, &op)   // one SMI per call
//   userland drives the transaction: begin(0x20) -> op -> end(0x24)
//
// This module is a DUMB single-SMI firer. The caller (arieltune) enforces the
// safety gates (BC-250 detection, APCB-slot guard).

#include <linux/module.h>
#include <linux/proc_fs.h>
#include <linux/uaccess.h>
#include <linux/io.h>
#include <linux/gfp.h>
#include <linux/mm.h>

#define DRV_NAME "smiflash"
#define SMIFLASH_MAXDATA 256

static unsigned short smi_port = 0xB0;
module_param(smi_port, ushort, 0444);
MODULE_PARM_DESC(smi_port, "platform SW-SMI command port (FADT SMI_CMD; BC-250 = 0xB0, NOT 0xB2)");

struct smiflash_buf {        // the 24-byte struct the SMM handler consumes
	uint64_t data;       // +0x00 phys addr of data buffer
	uint32_t offset;     // +0x08 field (pass chip offset directly)
	uint32_t size;       // +0x0C bytes
	uint8_t  status;     // +0x10 handler status (0=OK,1=err)
	uint8_t  pad[7];
} __attribute__((packed));

struct smiflash_op {         // userland <-> module ioctl payload (packed: stable
	uint8_t  cmd;        // layout for the Python probe -> "<B I I B I 256s")
	uint32_t offset;                 // chip offset
	uint32_t size;                   // bytes for the flash op
	uint8_t  status_out;             // returned handler status
	uint32_t dlen;                   // bytes of inline data (<= SMIFLASH_MAXDATA)
	uint8_t  data[SMIFLASH_MAXDATA]; // write: in; read: out
} __attribute__((packed));

#define SMIFLASH_IOC_MAGIC 'F'
#define SMIFLASH_DO        _IOWR(SMIFLASH_IOC_MAGIC, 1, struct smiflash_op)

static struct smiflash_buf *g_buf;
static unsigned long g_buf_pa;
static uint8_t *g_data;
static unsigned long g_data_pa;

// Fire one SMI: RBX = low32(buf_pa), RCX = high32(buf_pa), out cmd -> smi_port.
// register-asm forces the exact registers the handler reads (save-state 0x27/0x28).
static void fire(uint8_t cmd, uint64_t buf_pa, unsigned short port)
{
	register uint64_t r_rbx asm("rbx") = (uint32_t)buf_pa;
	register uint64_t r_rcx asm("rcx") = (uint32_t)(buf_pa >> 32);
	unsigned long flags;

	local_irq_save(flags);
	asm volatile("outb %%al, %%dx"
		     :: "a"(cmd), "d"(port), "r"(r_rbx), "r"(r_rcx) : "memory");
	local_irq_restore(flags);
}

static long smiflash_ioctl(struct file *f, unsigned int cmd, unsigned long arg)
{
	struct smiflash_op op;

	if (cmd != SMIFLASH_DO)
		return -ENOTTY;
	if (copy_from_user(&op, (void __user *)arg, sizeof(op)))
		return -EFAULT;
	if (op.dlen > SMIFLASH_MAXDATA)
		return -EINVAL;

	// stage inbound data (writes); poison otherwise so a non-read is visible
	if (op.dlen)
		memcpy(g_data, op.data, op.dlen);
	else
		memset(g_data, 0xAA, PAGE_SIZE);

	g_buf->data   = g_data_pa;
	g_buf->offset = op.offset;
	g_buf->size   = op.size;
	g_buf->status = 0xEE;

	fire(op.cmd, (uint64_t)g_buf_pa, smi_port);

	op.status_out = g_buf->status;
	// return read data inline (cap at MAXDATA)
	memcpy(op.data, g_data, SMIFLASH_MAXDATA);
	op.dlen = (op.size < SMIFLASH_MAXDATA) ? op.size : SMIFLASH_MAXDATA;

	if (copy_to_user((void __user *)arg, &op, sizeof(op)))
		return -EFAULT;
	return 0;
}

static const struct proc_ops smiflash_pops = {
	.proc_ioctl = smiflash_ioctl,
};

static struct proc_dir_entry *g_proc;

static struct page *alloc_dma32(unsigned long *pa)
{
	struct page *pg = alloc_pages(GFP_KERNEL | __GFP_DMA32 | __GFP_ZERO, 0);

	if (!pg)
		return NULL;
	if (page_to_phys(pg) >> 32) {     // SMM validator wants the buffer < 4GB
		__free_pages(pg, 0);
		return NULL;
	}
	*pa = page_to_phys(pg);
	return pg;
}

static int __init smiflash_init(void)
{
	struct page *bp, *dp;

	bp = alloc_dma32(&g_buf_pa);
	if (!bp)
		return -ENOMEM;
	g_buf = page_address(bp);

	dp = alloc_dma32(&g_data_pa);
	if (!dp) {
		__free_pages(bp, 0);
		return -ENOMEM;
	}
	g_data = page_address(dp);

	g_proc = proc_create(DRV_NAME, 0600, NULL, &smiflash_pops);
	if (!g_proc) {
		free_page((unsigned long)g_data);
		free_page((unsigned long)g_buf);
		return -ENOMEM;
	}
	pr_info(DRV_NAME ": loaded; smi_port=0x%02x cmd_pa=0x%lx data_pa=0x%lx\n",
		smi_port, g_buf_pa, g_data_pa);
	return 0;
}

static void __exit smiflash_exit(void)
{
	if (g_proc)
		proc_remove(g_proc);
	if (g_data)
		free_page((unsigned long)g_data);
	if (g_buf)
		free_page((unsigned long)g_buf);
	pr_info(DRV_NAME ": unloaded\n");
}

module_init(smiflash_init);
module_exit(smiflash_exit);
MODULE_LICENSE("GPL");
MODULE_DESCRIPTION("BC-250 SmiFlash: kernel -> SMM -> SPI-flash R/W primitive for owner firmware access / research");
MODULE_AUTHOR("Cachenetics");
