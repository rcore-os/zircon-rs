use super::*;
use alloc::vec::Vec;
use zircon_object::{
    dev::*,
    vm::{pages, VmObject},
};

impl Syscall<'_> {
    pub fn sys_pci_add_subtract_io_range(
        &self,
        handle: HandleValue,
        mmio: bool,
        base: u64,
        len: u64,
        add: bool,
    ) -> ZxResult {
        info!(
            "pci.add_subtract_io_range: handle_value={:#x}, mmio={:#}, base={:#x}, len={:#x}, add={:#}",
            handle, mmio, base, len, add
        );
        let proc = self.thread.proc();
        proc.get_object::<Resource>(handle)?
            .validate(ResourceKind::ROOT)?;
        let addr_space = if mmio {
            PciAddrSpace::MMIO
        } else {
            PciAddrSpace::PIO
        };
        if add {
            PCIeBusDriver::add_bus_region(base, len, addr_space)
        } else {
            PCIeBusDriver::sub_bus_region(base, len, addr_space)
        }
    }
    #[allow(unreachable_code, unused_variables, unused_mut)]
    pub fn sys_pci_cfg_pio_rw(
        &self,
        handle: HandleValue,
        bus: u8,
        dev: u8,
        func: u8,
        offset: u8,
        mut val: UserInOutPtr<u32>,
        width: usize,
        write: bool,
    ) -> ZxResult {
        #[cfg(not(target_arch = "x86_64"))]
        return Err(ZxError::NOT_SUPPORTED);
        #[cfg(target_arch = "x86_64")]
        {
            info!(
                "pci.sys_pci_cfg_pio_rw: handle_value={:#x}, bus={:#x}, dev={:#x}, func={:#x}, offset={:#x}, width={:#x}, write={:#}",
                handle, bus, dev, func, offset, width, write
            );
            let proc = self.thread.proc();
            proc.get_object::<Resource>(handle)?
                .validate(ResourceKind::ROOT)?;
            if write {
                let tmp = val.read()?;
                pio_config_write(bus, dev, func, offset, tmp, width)?;
            } else {
                let mut tmp = pio_config_read(bus, dev, func, offset, width)?;
                val.write(tmp)?;
            }
            Ok(())
        }
    }
    pub fn sys_pci_init(&self, handle: HandleValue, init_buf: usize, len: u32) -> ZxResult {
        info!(
            "pci.init: handle_value={:#x}, init_buf={:#x}, len={:#x}",
            handle, init_buf, len
        );
        let proc = self.thread.proc();
        proc.get_object::<Resource>(handle)?
            .validate(ResourceKind::ROOT)?;
        if len > PCI_INIT_ARG_MAX_SIZE as u32 {
            return Err(ZxError::INVALID_ARGS);
        }
        #[allow(non_snake_case)]
        let HEADER_SIZE = core::mem::size_of::<PciInitArgsHeader>();
        #[allow(non_snake_case)]
        let ADDR_WINDOWS_SIZE = core::mem::size_of::<PciInitArgsAddrWindows>();
        let arg_header_in: UserInPtr<PciInitArgsHeader> = init_buf.into();
        let mut arg_header = arg_header_in.read()?;
        let expected_len = HEADER_SIZE + arg_header.addr_window_count as usize * ADDR_WINDOWS_SIZE;
        if len != expected_len as u32 {
            return Err(ZxError::INVALID_ARGS);
        }
        let mut init_buf = init_buf + core::mem::size_of::<PciInitArgsHeader>();
        let mut addr_windows: Vec<PciInitArgsAddrWindows> = Vec::new();
        for _i in 0..arg_header.addr_window_count {
            let arg_win_in: UserInPtr<PciInitArgsAddrWindows> = init_buf.into();
            let arg_win = arg_win_in.read()?;
            addr_windows.push(arg_win);
            init_buf += ADDR_WINDOWS_SIZE;
        }
        // Configure interrupts
        pci_configure_interrupt(&mut arg_header)?;
        if arg_header.addr_window_count != 1 {
            return Err(ZxError::INVALID_ARGS); // for non DesignWare Controller
        }
        let addr_win = &mut addr_windows[0];
        if addr_win.bus_start != 0 || addr_win.bus_start > addr_win.bus_end {
            return Err(ZxError::INVALID_ARGS);
        }
        // Some systems will report overly large PCIe config regions
        // that collide with architectural registers.
        #[cfg(target_arch = "x86_64")]
        {
            let num_buses: u8 = addr_win.bus_end - addr_win.bus_start + 1;
            let mut end: u64 = addr_win.base + num_buses as u64 * PCIE_ECAM_BYTES_PER_BUS as u64;
            let high_limit: u64 = 0xfec00000;
            if end > high_limit {
                end = high_limit;
                if end < addr_win.base {
                    return Err(ZxError::INVALID_ARGS);
                }
                addr_win.size =
                    ((end - addr_win.base) & (PCIE_ECAM_BYTES_PER_BUS as u64 - 1)) as usize;
                let new_bus_end: usize =
                    addr_win.size / PCIE_ECAM_BYTES_PER_BUS + addr_win.bus_start as usize - 1;
                if new_bus_end as usize >= PCIE_MAX_BUSSES {
                    return Err(ZxError::INVALID_ARGS);
                }
                addr_win.bus_end = new_bus_end as u8;
            }
        }
        if addr_win.cfg_space_type == PCI_CFG_SPACE_TYPE_MMIO {
            if addr_win.size < PCIE_ECAM_BYTES_PER_BUS
                || addr_win.size / PCIE_ECAM_BYTES_PER_BUS
                    > PCIE_MAX_BUSSES - addr_win.bus_start as usize
            {
                return Err(ZxError::INVALID_ARGS);
            }
            let addr_provider = Arc::new(MmioPcieAddressProvider::new());
            addr_provider.add_ecam(PciEcamRegion {
                phys_base: addr_win.base,
                size: addr_win.size,
                bus_start: addr_win.bus_start,
                bus_end: addr_win.bus_end,
            })?;
            PCIeBusDriver::set_address_translation_provider(addr_provider)?;
        } else if addr_win.cfg_space_type == PCI_CFG_SPACE_TYPE_PIO {
            let addr_provider = Arc::new(PioPcieAddressProvider::new());
            PCIeBusDriver::set_address_translation_provider(addr_provider)?;
        } else {
            return Err(ZxError::INVALID_ARGS);
        }
        PCIeBusDriver::add_root(0, arg_header.dev_pin_to_global_irq)?;
        PCIeBusDriver::start_bus_driver()?;
        Ok(())
    }
    pub fn sys_pci_map_interrupt(
        &self,
        dev: HandleValue,
        irq: i32,
        mut out_handle: UserOutPtr<HandleValue>,
    ) -> ZxResult {
        info!("pci.map_interrupt: handle_value={:#x}, irq={:#x}", dev, irq);
        let proc = self.thread.proc();
        let dev = proc.get_object_with_rights::<PcieDeviceKObject>(dev, Rights::READ)?;
        let interrupt = dev.map_interrupt(irq)?;
        let handle = proc.add_handle(Handle::new(interrupt, Rights::DEFAULT_PCI_INTERRUPT));
        out_handle.write(handle)?;
        Ok(())
    }
    pub fn sys_pci_get_nth_device(
        &self,
        handle: HandleValue,
        index: u32,
        mut out_info: UserOutPtr<PcieDeviceInfo>,
        mut out_handle: UserOutPtr<HandleValue>,
    ) -> ZxResult {
        info!(
            "pci.get_nth_device: handle_value={:#x}, index={:#x}",
            handle, index,
        );
        let proc = self.thread.proc();
        proc.get_object::<Resource>(handle)?
            .validate(ResourceKind::ROOT)?;
        let (info, kobject) = PCIeBusDriver::get_nth_device(index as usize)?;
        out_info.write(info)?;
        let handle = proc.add_handle(Handle::new(Arc::new(kobject), Rights::DEFAULT_DEVICE));
        out_handle.write(handle)?;
        Ok(())
    }
    pub fn sys_pci_get_bar(
        &self,
        handle: HandleValue,
        bar_num: u32,
        mut out_bar: UserOutPtr<PciBar>,
        mut out_handle: UserOutPtr<HandleValue>,
    ) -> ZxResult {
        info!(
            "pci.get_bar: handle_value={:#x}, bar_num={:#x}",
            handle, bar_num,
        );
        let proc = self.thread.proc();
        let devobj =
            proc.get_object_with_rights::<PcieDeviceKObject>(handle, Rights::READ | Rights::WRITE)?;
        let info = devobj.get_bar(bar_num)?;
        let mut bar = PciBar {
            id: 0,
            size: info.size as usize,
            bar_type: if info.is_mmio { 1 } else { 2 },
            addr: 0,
        };
        if info.is_mmio {
            let vmo = VmObject::new_physical(info.bus_addr as usize, pages(info.size as usize));
            let handle = proc.add_handle(Handle::new(vmo, Rights::DEFAULT_VMO));
            out_handle.write(handle)?;
            devobj.device.device().unwrap().enable_mmio(true)?;
        } else {
            bar.addr = info.bus_addr;
            devobj.device.device().unwrap().enable_pio(true)?;
        }
        out_bar.write(bar)?;
        Ok(())
    }
    pub fn sys_pci_enable_bus_master(&self, handle: HandleValue, enable: bool) -> ZxResult {
        info!("pci.get_bar: handle_value={:#x}, enable={}", handle, enable,);
        let proc = self.thread.proc();
        let devobj = proc.get_object_with_rights::<PcieDeviceKObject>(handle, Rights::WRITE)?;
        devobj.device.device().unwrap().enable_master(enable)
    }
    pub fn sys_pci_query_irq_mode(
        &self,
        handle: HandleValue,
        mode: u32,
        mut out_max_irqs: UserOutPtr<u32>,
    ) -> ZxResult {
        info!(
            "pci.query_irq_mode: handle_value={:#x}, mode={:#x}",
            handle, mode
        );
        let proc = self.thread.proc();
        let devobj = proc.get_object_with_rights::<PcieDeviceKObject>(handle, Rights::READ)?;
        match devobj
            .device
            .device()
            .unwrap()
            .get_irq_mode_capabilities(mode)
        {
            Ok(x) => {
                out_max_irqs.write(x.max_irqs)?;
                Ok(())
            }
            Err(err) => Err(err),
        }
    }
    pub fn sys_pci_set_irq_mode(
        &self,
        handle: HandleValue,
        mode: u32,
        requested_irq_count: u32,
    ) -> ZxResult {
        info!(
            "pci.set_irq_mode: handle_value={:#x}, mode={:#x}, requested_irq_count={:#x}",
            handle, mode, requested_irq_count
        );
        let proc = self.thread.proc();
        let devobj = proc.get_object_with_rights::<PcieDeviceKObject>(handle, Rights::WRITE)?;
        devobj
            .device
            .device()
            .unwrap()
            .set_irq_mode(mode, requested_irq_count)
    }

    pub fn sys_pci_config_read(
        &self,
        handle: HandleValue,
        offset: usize,
        width: usize,
        mut out_val: UserOutPtr<u32>,
    ) -> ZxResult {
        info!(
            "pci.config_read: handle={:#x}, offset={:x}, width={:x}",
            handle, offset, width
        );
        let proc = self.thread.proc();
        let devobj =
            proc.get_object_with_rights::<PcieDeviceKObject>(handle, Rights::READ | Rights::WRITE)?;
        let val = devobj.device.device().unwrap().config_read(offset, width)?;
        out_val.write(val)?;
        Ok(())
    }

    pub fn sys_pci_config_write(
        &self,
        handle: HandleValue,
        offset: usize,
        width: usize,
        val: u32,
    ) -> ZxResult {
        info!(
            "pci.config_write: handle={:#x}, offset={:x}, width={:x}",
            handle, offset, width
        );
        let proc = self.thread.proc();
        let devobj =
            proc.get_object_with_rights::<PcieDeviceKObject>(handle, Rights::READ | Rights::WRITE)?;
        devobj
            .device
            .device()
            .unwrap()
            .config_write(offset, width, val)?;
        Ok(())
    }
}

#[repr(C)]
pub struct PciBar {
    pub id: u32,
    pub bar_type: u32,
    pub size: usize,
    pub addr: u64,
}