# Build the smiflash kernel module out-of-tree.
#
# On a normal distro with complete kernel headers this Just Works. On a BC-250
# whose headers package ships incomplete (missing autoconf.h) and/or with host
# tools marked x86-64-v4 that the Zen2 CPU can't start, run `./prepare.sh` first
# (the DKMS PRE_BUILD hook does this automatically) to make the tree buildable
# on the board itself — no cross-build host needed.

obj-m := smiflash.o

KDIR ?= /lib/modules/$(shell uname -r)/build

all:
	$(MAKE) -C $(KDIR) M=$(CURDIR) modules

clean:
	$(MAKE) -C $(KDIR) M=$(CURDIR) clean 2>/dev/null || rm -f *.o *.ko *.mod* .*.cmd modules.order Module.symvers
