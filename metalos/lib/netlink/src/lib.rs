/*
 * Copyright (c) Facebook, Inc. and its affiliates.
 *
 * This source code is licensed under the MIT license found in the
 * LICENSE file in the root directory of this source tree.
 */
use std::ffi::CStr;
use std::ffi::CString;
use std::fmt;
use std::marker::PhantomData;

use anyhow::bail;
use anyhow::Context;
use anyhow::Error;
use anyhow::Result;
use bitflags::bitflags;
use derive_more::Display;
use netlink_sys::nl_addr2str;
use netlink_sys::nl_cache;
use netlink_sys::nl_cache_get_first;
use netlink_sys::nl_cache_get_next;
use netlink_sys::nl_cache_put;
use netlink_sys::nl_close;
use netlink_sys::nl_connect;
use netlink_sys::nl_geterror;
use netlink_sys::nl_sock;
use netlink_sys::nl_socket_alloc;
use netlink_sys::nl_socket_free;
use netlink_sys::rtnl_link;
use netlink_sys::rtnl_link_add;
use netlink_sys::rtnl_link_alloc;
use netlink_sys::rtnl_link_alloc_cache;
use netlink_sys::rtnl_link_change;
use netlink_sys::rtnl_link_delete;
use netlink_sys::rtnl_link_get_addr;
use netlink_sys::rtnl_link_get_flags;
use netlink_sys::rtnl_link_get_ifindex;
use netlink_sys::rtnl_link_get_name;
use netlink_sys::rtnl_link_put;
use netlink_sys::rtnl_link_set_flags;
use netlink_sys::rtnl_link_set_name;
use netlink_sys::rtnl_link_set_type;
use netlink_sys::rtnl_link_unset_flags;
use netlink_sys::AF_UNSPEC;
use netlink_sys::IFF_UP;
use netlink_sys::NETLINK_ROUTE;
use netlink_sys::NETLINK_XFRM;
use netlink_sys::NLM_F_ACK;
use netlink_sys::NLM_F_CREATE;
use netlink_sys::NLM_F_EXCL;
use netlink_sys::NLM_F_REQUEST;
use nix::errno::errno;
use num_derive::FromPrimitive;
use num_derive::ToPrimitive;

/// Format an error message from a failed libnl* call.
fn nlerrmsg(err: i32, msg: &str) -> String {
    format!("{}: {}", msg, unsafe {
        CStr::from_ptr(nl_geterror(err)).to_string_lossy()
    },)
}

// Underlying socket structure used for all netlink(3) operations.
struct NlSocket(
    // WARNING: Do not attempt to add Clone or Copy trait support because
    // this structure references dynamically allocated C structures.
    *mut nl_sock,
);

/// Management protocols supported by the netlink.
#[derive(Clone, Copy, FromPrimitive, ToPrimitive, Display)]
#[repr(u32)]
enum NlProtocols {
    Route = NETLINK_ROUTE,
    IPsec = NETLINK_XFRM,
    Invalid = 0xFFFFFFFF,
}

impl NlSocket {
    /// Allocate a new (unconnected) netlink socket.
    /// Must be connect()ed before use.
    fn new() -> Result<Self> {
        let ns_socket = unsafe { nl_socket_alloc() };
        match ns_socket.is_null() {
            true => bail!("nl_socket_alloc() failed: {}", errno()),
            false => Ok(Self(ns_socket)),
        }
    }

    /// Connect to specific netlink management protocol.
    /// A connection is required for all netlink(3) operations.
    fn connect(self, protocol: NlProtocols) -> Result<NlConnectedSocket> {
        let nlerr = unsafe { nl_connect(self.0, protocol as i32) };
        if nlerr != 0 {
            let msg = format!("nl_connect() failed for protocol: {}", protocol);
            bail!(nlerrmsg(nlerr, &msg));
        }
        Ok(NlConnectedSocket(self))
    }

    /// Get nl_sock pointer reference.
    fn nl_sock(&self) -> &*mut nl_sock {
        &self.0
    }
}

impl Drop for NlSocket {
    /// Cleanup a NlSocket.
    fn drop(&mut self) {
        unsafe { nl_socket_free(self.0) };
    }
}

struct NlConnectedSocket(
    // WARNING: Do not attempt to add Clone or Copy trait support because
    // this structure references dynamically allocated C structures.
    NlSocket,
);

impl NlConnectedSocket {
    /// Get nl_sock pointer reference.
    fn nl_sock(&self) -> &*mut nl_sock {
        self.0.nl_sock()
    }
}

impl Drop for NlConnectedSocket {
    /// Cleanup a NlSocket.
    fn drop(&mut self) {
        unsafe { nl_close(*self.nl_sock()) };
    }
}

/// Netlink routing query and management interfaces.
pub struct NlRoutingSocket(
    // WARNING: Do not attempt to add Clone or Copy trait support because
    // this structure references dynamically allocated C structures.
    NlConnectedSocket,
);

impl NlRoutingSocket {
    /// Allocate a new netlink routing socket.
    pub fn new() -> Result<Self> {
        let sock = NlSocket::new().context("Failed to create netlink routing socket.")?;
        let connected_sock = sock
            .connect(NlProtocols::Route)
            .context("Failed to create netlink routing socket.")?;
        Ok(Self(connected_sock))
    }

    /// Get nl_sock pointer reference.
    fn nl_sock(&self) -> &*mut nl_sock {
        self.0.nl_sock()
    }

    // Add link
    pub fn add_link(&self, name: String, interface_type: String) -> Result<()> {
        let c_int_str =
            CString::new(name.as_bytes()).expect("Failed to create CStr for interface rename");
        let c_int_type = CString::new(interface_type.as_bytes())
            .expect("Failed to create CStr for interface type");

        let new = RtnlLink::new().context("Failed to create new RtnlLink")?;
        unsafe { rtnl_link_set_name(new.0, c_int_str.as_ptr()) };
        unsafe { rtnl_link_set_type(new.0, c_int_type.as_ptr()) };
        let flags = RtnlLinkAddFlags::NLM_F_EXCL
            | RtnlLinkAddFlags::NLM_F_CREATE
            | RtnlLinkAddFlags::NLM_F_REQUEST
            | RtnlLinkAddFlags::NLM_F_ACK;
        let nlerr = unsafe { rtnl_link_add(*self.0.nl_sock(), new.0, flags.bits) };
        if nlerr != 0 {
            let msg = format!("add_link() failed: Netlink error {}", nlerr);
            return Err(Error::msg(msg));
        }
        Ok(())
    }
}

bitflags! {
    /// State flags associated with an Rtnl*Link struct.
    pub struct RtnlLinkFlags: u32 {
        const UP = IFF_UP;
    }

    pub struct RtnlLinkAddFlags: i32 {
        const NLM_F_EXCL = NLM_F_EXCL as i32;
        const NLM_F_CREATE = NLM_F_CREATE as i32;
        const NLM_F_REQUEST = NLM_F_REQUEST as i32;
        const NLM_F_ACK = NLM_F_ACK as i32;
    }
}

// Prevent users from getting access to rl_link pointers.
mod private {
    pub trait Sealed {}
    impl Sealed for super::RtnlLink {}
    impl<'cache> Sealed for super::RtnlCachedLink<'cache> {}
}

/// Sealed trait for accessing Rtnl*Link information. This trait is sealed
/// to prevent consumers from using or implementing these interfaces.
pub trait RtnlLinkPrivate: private::Sealed {
    /// Get rl_link pointer reference.
    #[doc(hidden)]
    fn rl_link(&self) -> &*mut rtnl_link;

    /// Get interface flags.
    #[doc(hidden)]
    fn get_flags(&self) -> RtnlLinkFlags {
        RtnlLinkFlags {
            bits: unsafe { rtnl_link_get_flags(*self.rl_link()) },
        }
    }
}

/// A public trait for accessing Rtnl*Link information.
pub trait RtnlLinkCommon {
    /// Lookup the link index.
    fn index(&self) -> i32;

    /// Lookup the link name, if any.
    fn name(&self) -> Option<String>;

    /// Lookup the link address and attempt to decode it.
    fn mac_addr(&self) -> Option<String>;

    /// Check if an interface is up.
    fn is_up(&self) -> bool;

    /// Check if an interface is down.
    fn is_down(&self) -> bool {
        !self.is_up()
    }

    /// Base std::fmt::Display trait implementation.
    fn display(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let link_name = self.name().unwrap_or_else(|| "<unknown>".to_string());
        let link_addr = self.mac_addr().unwrap_or_else(|| "<unknown>".to_string());
        write!(
            f,
            "{} (index: {}, addr: {})",
            link_name,
            self.index(),
            link_addr
        )
    }
}

impl<T: RtnlLinkPrivate> RtnlLinkCommon for T {
    fn index(&self) -> i32 {
        unsafe { rtnl_link_get_ifindex(*self.rl_link()) }
    }

    fn name(&self) -> Option<String> {
        let c_name = unsafe { rtnl_link_get_name(*self.rl_link()) };
        match c_name.is_null() {
            true => None,
            false => Some(unsafe { CStr::from_ptr(c_name).to_string_lossy().into_owned() }),
        }
    }

    fn mac_addr(&self) -> Option<String> {
        let c_addr = unsafe { rtnl_link_get_addr(*self.rl_link()) };
        let mut c_buf = [0i8; 32];
        let addr_cstr = unsafe {
            let c_buf_ptr = c_buf.as_mut_ptr();
            CStr::from_ptr(nl_addr2str(c_addr, c_buf_ptr, 24))
        };
        let addr_str = addr_cstr
            .to_str()
            .expect("mac address was not valid utf-8")
            .to_string();

        match addr_str.chars().count() == 17 {
            true => Some(addr_str),
            false => None,
        }
    }

    fn is_up(&self) -> bool {
        self.get_flags().contains(RtnlLinkFlags::UP)
    }
}

/// A dynamically allocated netlink routing link.
struct RtnlLink(
    // WARNING: Do not attempt to add Clone or Copy trait support because
    // this structure references dynamically allocated C structures.
    *mut rtnl_link,
);

impl RtnlLink {
    /// Allocate an empty RtnlLink structure.
    /// Used for routing link property updates.
    fn new() -> Result<Self> {
        let rl_link = unsafe { rtnl_link_alloc() };
        match rl_link.is_null() {
            true => bail!("rtnl_link_alloc() failed: {}", errno()),
            false => Ok(Self(rl_link)),
        }
    }
}

impl Drop for RtnlLink {
    /// Cleanup a RtnlLink.
    fn drop(&mut self) {
        unsafe { rtnl_link_put(self.0) };
    }
}

impl RtnlLinkPrivate for RtnlLink {
    fn rl_link(&self) -> &*mut rtnl_link {
        &self.0
    }
}

impl fmt::Display for RtnlLink {
    /// Print RtnlLink information.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.display(f)
    }
}

/// A cached allocated netlink routing link.
pub struct RtnlCachedLink<'cache>(
    // WARNING: Do not attempt to add Clone or Copy trait support because
    // this structure references dynamically allocated C structures.
    *mut rtnl_link,
    PhantomData<&'cache ()>,
);

/// A trait providing private methods for accessing RtnlLink information.
impl<'cache> RtnlCachedLink<'cache> {
    /// Set the link up/down state.
    fn update_flags(&self, sock: &NlRoutingSocket, flags: RtnlLinkFlags, set: bool) -> Result<()> {
        let op = match set {
            true => "set",
            false => "clear",
        };
        let cmsg = format!(
            "Failed to {} link state flags {:#x} for link {}",
            op, flags.bits, self
        );
        let change = RtnlLink::new().context(cmsg.clone())?;
        unsafe {
            if set {
                rtnl_link_set_flags(change.0, flags.bits);
            } else {
                rtnl_link_unset_flags(change.0, flags.bits);
            }
        };
        let nlerr = unsafe { rtnl_link_change(*sock.nl_sock(), self.0, change.0, 0) };
        if nlerr != 0 {
            return Err(Error::msg(nlerrmsg(nlerr, "rtnl_link_change() failed"))).context(cmsg);
        }
        Ok(())
    }
    /// Set the link name.
    fn update_name(&self, sock: &NlRoutingSocket, name: &str) -> Result<()> {
        let cmsg = format!("Failed to rename link {}", self);

        let change = RtnlLink::new().context(cmsg.clone())?;

        let c_int_str =
            CString::new(name.as_bytes()).expect("Failed to create CStr for interface rename");

        unsafe { rtnl_link_set_name(change.0, c_int_str.as_ptr()) };
        let nlerr = unsafe { rtnl_link_change(*sock.nl_sock(), self.0, change.0, 0) };
        if nlerr != 0 {
            return Err(Error::msg(nlerrmsg(nlerr, "rtnl_link_change() failed"))).context(cmsg);
        }
        Ok(())
    }
}

impl<'cache> RtnlLinkPrivate for RtnlCachedLink<'cache> {
    fn rl_link(&self) -> &*mut rtnl_link {
        &self.0
    }
}

impl<'cache> fmt::Display for RtnlCachedLink<'cache> {
    /// Print RtnlLink information.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.display(f)
    }
}

/// A public trait for accessing RtnlCachedLink information.
pub trait RtnlCachedLinkTrait: RtnlLinkCommon {
    /// Delete/remove a virtual interface
    fn delete(&self, _sock: &NlRoutingSocket) -> Result<()>;

    /// Set the interface state to up.
    fn set_up(&self, _sock: &NlRoutingSocket) -> Result<()>;

    /// Set the interface state to down.
    fn set_down(&self, _sock: &NlRoutingSocket) -> Result<()>;

    /// Set the interface name.
    fn set_name(&self, _sock: &NlRoutingSocket, name: &str) -> Result<()>;
}

impl<'cache> RtnlCachedLinkTrait for RtnlCachedLink<'cache> {
    fn delete(&self, sock: &NlRoutingSocket) -> Result<()> {
        let nlerr = unsafe { rtnl_link_delete(*sock.nl_sock(), *self.rl_link()) };
        if nlerr > 0 {
            let msg = format!("rtnl_link_del() failed: Netlink error {}", nlerr);
            return Err(Error::msg(msg));
        }
        Ok(())
    }

    fn set_up(&self, sock: &NlRoutingSocket) -> Result<()> {
        self.update_flags(sock, RtnlLinkFlags::UP, true)
            .context(format!("Failed to set link state up for link {}", self))
    }

    fn set_down(&self, sock: &NlRoutingSocket) -> Result<()> {
        self.update_flags(sock, RtnlLinkFlags::UP, false)
            .context(format!("Failed to set link state down for link {}", self))
    }

    fn set_name(&self, sock: &NlRoutingSocket, name: &str) -> Result<()> {
        self.update_name(sock, name)
            .context(format!("Failed to change link name for {}", self))
    }
}

/// A netlink routing link cache for querying link information.
pub struct RtnlLinkCache<'a> {
    // WARNING: Do not attempt to add Clone or Copy trait support because
    // this structure references dynamically allocated C structures.
    rlc_cache: *mut nl_cache,
    rlc_links: Vec<RtnlCachedLink<'a>>,
}

impl<'a> RtnlLinkCache<'a> {
    /// Create a RtnlLinkCache. Used for querying RtnlCachedLink information.
    pub fn new(sock: &NlRoutingSocket) -> Result<Self> {
        let mut rlc_cache = std::ptr::null_mut();
        let family: i32 = AF_UNSPEC as i32;
        let nlerr = unsafe { rtnl_link_alloc_cache(*sock.nl_sock(), family, &mut rlc_cache) };
        if nlerr != 0 {
            let msg = format!("rtnl_link_alloc_cache() failed for family: {}", family);
            return Err(Error::msg(nlerrmsg(nlerr, &msg)))
                .context("Failed to create netlink link cache");
        }

        // We preallocate all the RtnlCachedLink structures contained in this
        // cache since their lifetimes are constrained by the lifetime of
        // this cache.
        Ok(Self {
            rlc_cache,
            rlc_links: RtnlLinkCache::get_links(rlc_cache),
        })
    }

    fn get_links(rlc_cache: *mut nl_cache) -> Vec<RtnlCachedLink<'a>> {
        let mut rlc_links: Vec<RtnlCachedLink<'a>> = vec![];
        let mut i = unsafe { nl_cache_get_first(rlc_cache) };
        while !i.is_null() {
            let rl = RtnlCachedLink(i as *mut rtnl_link, PhantomData);
            rlc_links.push(rl);
            i = unsafe { nl_cache_get_next(i) };
        }
        rlc_links
    }

    /// Get RtnlCachedLink structs contained within this cache.
    pub fn links(&self) -> &Vec<RtnlCachedLink<'a>> {
        &self.rlc_links
    }
}

impl<'a> Drop for RtnlLinkCache<'a> {
    /// Cleanup a RtnlLinkCache.
    fn drop(&mut self) {
        unsafe { nl_cache_put(self.rlc_cache) };
    }
}

#[cfg(test)]
mod tests {
    use metalos_macros::test;
    use metalos_macros::vmtest;
    use rand::Rng;

    use super::*;

    #[test]
    /// Test allocating a NlSocket without a connection.
    fn test_no_connect_nlsocket() -> Result<()> {
        NlSocket::new()?;
        Ok(())
    }

    #[test]
    /// Test an invalid NlSocket connection request.
    fn test_failed_connect_nlsocket() -> Result<()> {
        let sock = NlSocket::new()?;
        assert!(sock.connect(NlProtocols::Invalid).is_err());
        Ok(())
    }

    #[test]
    /// Test RtnlLinkCache allocation with an invalid NlSocket.
    fn test_failed_rtlink_cache() -> Result<()> {
        let sock = NlSocket::new()?;
        let csock = sock.connect(NlProtocols::IPsec)?;
        let rsock = NlRoutingSocket(csock);
        assert!(RtnlLinkCache::new(&rsock).is_err());
        Ok(())
    }

    #[test]
    /// Test RtnlLinkCache allocation.
    fn test_iterate_rtlinks() -> Result<()> {
        let rsock = NlRoutingSocket::new()?;
        let rlc = RtnlLinkCache::new(&rsock)?;
        assert!(rlc.links().iter().count() > 0);
        Ok(())
    }

    #[test]
    /// Test RtnlCachedLink Display trait implementation.
    fn test_rtlink_cached_format() -> Result<()> {
        let rsock = NlRoutingSocket::new()?;
        let rlc = RtnlLinkCache::new(&rsock)?;
        for link in rlc.links() {
            format!("{}", link);
        }
        Ok(())
    }

    #[test]
    /// Test RtnlLink default Display trait implementation.
    fn test_rtlink_format() -> Result<()> {
        let link = RtnlLink::new()?;
        format!("{}", link);
        Ok(())
    }

    #[vmtest]
    /// Test bouncing the loopback interface (ie, taking it down and
    /// bringing it back up) within a vm.
    fn test_bounce_loopback() -> Result<()> {
        let rsock = NlRoutingSocket::new()?;
        let rlc = RtnlLinkCache::new(&rsock)?;

        // Find the loopback interface and verity that it's up.
        let lo = rlc
            .links()
            .iter()
            .find(|j| j.name().unwrap_or_else(|| "".to_string()) == "lo")
            .unwrap();
        assert!(lo.is_up());

        // Take down the loopback interface.
        lo.set_down(&rsock)?;
        let rlc2 = RtnlLinkCache::new(&rsock)?;
        let lo2 = rlc2
            .links()
            .iter()
            .find(|j| j.name().unwrap_or_else(|| "".to_string()) == "lo")
            .unwrap();
        assert!(lo2.is_down());

        // Bring the loopback interface back up.
        lo.set_up(&rsock)?;
        let rlc2 = RtnlLinkCache::new(&rsock)?;
        let lo2 = rlc2
            .links()
            .iter()
            .find(|j| j.name().unwrap_or_else(|| "".to_string()) == "lo")
            .unwrap();
        assert!(lo2.is_up());

        Ok(())
    }

    #[vmtest]
    /// Test renaming a virtual interface in a VM
    fn test_rename_interface() -> Result<()> {
        // Create Interface with random index to support stress testing
        let mut rng = rand::thread_rng();
        let rand_int_index: i16 = rng.gen_range(0..99);
        let int_name = format!("unittest{}", rand_int_index);
        let rsock = NlRoutingSocket::new()?;
        rsock.add_link(int_name.to_string(), "dummy".to_string())?;

        // Find new interface + rename
        let rlc = RtnlLinkCache::new(&rsock)?;
        let new_interface = rlc
            .links()
            .iter()
            .find(|j| j.name().unwrap_or_else(|| "".to_string()) == int_name)
            .unwrap();
        assert!(new_interface.is_down());

        // Ensure we get a different reandom int
        let mut rand_int_index2: i16 = rng.gen_range(0..99);
        while rand_int_index2 == rand_int_index {
            rand_int_index2 = rng.gen_range(0..99);
        }

        // Rename interface to new semi random name
        let int_name2 = format!("unittest{}", rand_int_index2);
        new_interface.set_name(&rsock, int_name2.as_str())?;

        // Find new interface + delete
        let rlc2 = RtnlLinkCache::new(&rsock)?;
        let new_interface2 = rlc2
            .links()
            .iter()
            .find(|j| j.name().unwrap_or_else(|| "".to_string()) == int_name2)
            .unwrap();
        assert!(new_interface2.is_down());
        new_interface2.delete(&rsock)?;

        // Ensure interface was deleted
        let rlc3 = RtnlLinkCache::new(&rsock)?;
        for link in rlc3.links().iter() {
            if link.name().unwrap_or_else(|| "".to_string()) == int_name2 {
                let emsg = format!("{} was found! It should be deleted. Debug.", int_name2);
                return Err(Error::msg(emsg));
            }
        }

        Ok(())
    }
}
