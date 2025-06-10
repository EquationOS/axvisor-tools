* compile
```bash
CROSS_COMPILE=aarch64-linux-gnu- ARCH=arm64 KDIR=~/workspace/Linux/linux-5.10.198/ make clean
```

* copy to guest
```bash
scp -P 5555 axvisor.ko root@localhost
```

* test
```bash
scp -P 5555 test_axvisor.c root@localhost
ssh -p 5555 root@localhost
insmod axvisor.ko
gcc test_axvisor.c -o test_axvisor
./test_axvisor
```


