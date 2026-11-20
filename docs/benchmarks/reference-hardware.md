# Reference hardware

Recorded 2026-11-20 from the owner's system report. Host and user names are deliberately not recorded.

## RH-OWNER

| Item | Value |
|---|---|
| Type | Laptop (MSI Katana GF76 11UD) |
| CPU | Intel Core i7-11800H, 8 cores / 16 threads, up to 4.6 GHz |
| RAM | 16 GiB DDR4-3200 (2 × 8 GiB, dual channel) |
| System disk | Kingston SFYRD2000G, 2 TB NVMe, PCIe 4.0 x4 |
| Second disk | 465 GB NVMe over USB (UAS), not used for benchmarks |
| Graphics | Intel UHD Graphics (iGPU), plus a discrete NVIDIA GPU (the 11UD model ships with one; not shown in the report) |
| Display | 17.3" 1920×1080 |
| OS | Windows 11 Home Single Language, build 26100 (24H2) |

## Consequences for the benchmark plan

- **Battery:** it is a laptop, so scenario S8 (on battery) is possible. Record the power plan and AC/battery state for every run.
- **VMs:** Windows 11 *Home* has neither Hyper-V nor Windows Sandbox. VM-LOW and VM-CLEAN use **VirtualBox** (or VMware Workstation Player). VM images need their own Windows licenses or evaluation images.
- **Hybrid graphics:** UI startup can depend on which GPU the toolkit initializes. Pin the app to the iGPU (Windows Graphics settings → Power saving) for footprint runs, and note it in the summary.
- **Windows 10 coverage:** the host runs Windows 11 24H2; Windows 10 22H2 is tested only in VM-CLEAN.
- **Fast host:** 16 GiB and a PCIe 4.0 SSD make memory pressure and hard faults rare on the host itself. Pressure scenarios (S5, W-PRESSURE) run in VM-LOW with 4–8 GiB.
