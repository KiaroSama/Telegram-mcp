use std::net::{IpAddr, Ipv4Addr, TcpListener};

pub fn nonloopback_address() -> IpAddr {
    addresses()
        .into_iter()
        .filter(|ip| !ip.is_loopback() && !ip.is_unspecified() && !ip.is_multicast())
        .find(|ip| TcpListener::bind((IpAddr::V4(*ip), 0)).is_ok())
        .map(IpAddr::V4)
        .expect("This test requires a bindable non-loopback IPv4 interface")
}

#[cfg(unix)]
fn addresses() -> Vec<Ipv4Addr> {
    struct Interfaces(*mut libc::ifaddrs);
    impl Drop for Interfaces {
        fn drop(&mut self) {
            // SAFETY: getifaddrs owns this list until freeifaddrs releases it.
            unsafe { libc::freeifaddrs(self.0) };
        }
    }
    let mut head = std::ptr::null_mut();
    // SAFETY: the output pointer lives until getifaddrs returns.
    assert_eq!(unsafe { libc::getifaddrs(&mut head) }, 0);
    let list = Interfaces(head);
    let mut next = list.0;
    let mut result = Vec::new();
    // SAFETY: the OS list and its socket addresses remain live while list is held.
    while let Some(entry) = unsafe { next.as_ref() } {
        if !entry.ifa_addr.is_null()
            && unsafe { (*entry.ifa_addr).sa_family as i32 } == libc::AF_INET
        {
            let addr = unsafe { &*entry.ifa_addr.cast::<libc::sockaddr_in>() };
            result.push(Ipv4Addr::from(addr.sin_addr.s_addr.to_ne_bytes()));
        }
        next = entry.ifa_next;
    }
    result
}

#[cfg(windows)]
fn addresses() -> Vec<Ipv4Addr> {
    use windows_sys::Win32::{
        Foundation::ERROR_BUFFER_OVERFLOW,
        NetworkManagement::IpHelper::{
            GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST,
            GetAdaptersAddresses, IP_ADAPTER_ADDRESSES_LH,
        },
        Networking::WinSock::{AF_INET, SOCKADDR_IN},
    };
    let mut bytes = 16_384_u32;
    for _ in 0..3 {
        let mut storage = vec![0_u64; (bytes as usize).div_ceil(8)];
        let head = storage.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
        // SAFETY: storage is aligned and contains at least bytes writable bytes.
        let result = unsafe {
            GetAdaptersAddresses(
                u32::from(AF_INET),
                GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER,
                std::ptr::null(),
                head,
                &mut bytes,
            )
        };
        if result == ERROR_BUFFER_OVERFLOW {
            assert!(
                bytes <= 16 * 1024 * 1024,
                "Adapter list exceeded its byte budget"
            );
            continue;
        }
        assert_eq!(result, 0, "GetAdaptersAddresses failed");
        let mut result = Vec::new();
        let mut next = head;
        // SAFETY: GetAdaptersAddresses returns lists backed by storage, still held here.
        while let Some(adapter) = unsafe { next.as_ref() } {
            let mut unicast = adapter.FirstUnicastAddress;
            while let Some(entry) = unsafe { unicast.as_ref() } {
                let address = entry.Address.lpSockaddr;
                if !address.is_null()
                    && entry.Address.iSockaddrLength as usize >= size_of::<SOCKADDR_IN>()
                    && unsafe { (*address).sa_family } == AF_INET
                {
                    let addr = unsafe { &*address.cast::<SOCKADDR_IN>() };
                    result.push(Ipv4Addr::from(
                        unsafe { addr.sin_addr.S_un.S_addr }.to_ne_bytes(),
                    ));
                }
                unicast = entry.Next;
            }
            next = adapter.Next;
        }
        return result;
    }
    panic!("Network interfaces changed during all three adapter queries");
}
