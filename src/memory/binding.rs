//! Memory binding

// Main docs: https://hwloc.readthedocs.io/en/v2.9/group__hwlocality__membinding.html

use crate::{ffi, Topology};
use bitflags::bitflags;
use derive_more::Display;
use errno::{errno, Errno};
use libc::{EINVAL, ENOMEM, ENOSYS, EXDEV};
use num_enum::{IntoPrimitive, TryFromPrimitive};
use std::{
    borrow::{Borrow, BorrowMut},
    ffi::{c_int, c_void},
    fmt::{self, Debug},
    mem::MaybeUninit,
    ops::{Deref, DerefMut},
    ptr::NonNull,
};
use thiserror::Error;

bitflags! {
    /// Memory binding flags.
    ///
    /// These flags can be used to refine the binding policy. All flags can be
    /// logically OR'ed together with the exception of `PROCESS` and `THREAD`;
    /// these two flags are mutually exclusive.
    ///
    /// Not all systems support all kinds of binding.
    /// [`Topology::support().memory_binding()`] may be used to query the
    /// actual memory binding support in the currently used operating system.
    #[repr(C)]
    #[doc(alias = "hwloc_membind_flags_t")]
    pub struct MemoryBindingFlags: c_int {
        /// Set policy for all threads of the specified (possibly multithreaded) process
        ///
        /// This flag is mutually exclusive with `THREAD`.
        #[doc(alias = "HWLOC_MEMBIND_PROCESS")]
        const PROCESS = (1<<0);

        /// Set policy for a specific thread of the current process
        ///
        /// This flag is mutually exclusive with `PROCESS`.
        #[doc(alias = "HWLOC_MEMBIND_THREAD")]
        const THREAD = (1<<1);

        /// Request strict binding from the OS
        ///
        /// If this flag is set, a binding function will fail if the binding can
        /// not be guaranteed or completely enforced. Otherwise, hwloc will
        /// attempt to achieve an approximation of the requested binding (e.g.
        /// targeting more or less threads and NUMA nodes).
        ///
        /// This flag has slightly different meanings depending on which
        /// function it is used with.
        #[doc(alias = "HWLOC_MEMBIND_STRICT")]
        const STRICT = (1<<2);

        /// Migrate existing allocated memory
        ///
        /// If the memory cannot be migrated and the `STRICT` flag is set, an
        /// error will be returned.
        #[doc(alias = "HWLOC_MEMBIND_MIGRATE")]
        const MIGRATE = (1<<3);

        /// Avoid any effect on CPU binding
        ///
        /// On some operating systems, some underlying memory binding
        /// functions also bind the application to the corresponding CPU(s).
        /// Using this flag will cause hwloc to avoid using OS functions that
        /// could potentially affect CPU bindings.
        ///
        /// Note, however, that using this flag may reduce hwloc's overall
        /// memory binding support.
        #[doc(alias = "HWLOC_MEMBIND_NOCPUBIND")]
        const NO_CPU_BINDING = (1<<4);

        /// Consider the bitmap argument as a nodeset.
        ///
        /// The bitmap argument is considered a nodeset if this flag is given,
        /// or a cpuset otherwise by default.
        ///
        /// Memory binding by CPU set cannot work for CPU-less NUMA memory nodes.
        /// Binding by nodeset should therefore be preferred whenever possible.
        #[doc(hidden)]
        #[doc(alias = "HWLOC_MEMBIND_BYNODESET")]
        const BY_NODE_SET = (1<<5);
    }
}
//
impl MemoryBindingFlags {
    /// Truth that these flags are in a valid state
    pub(crate) fn is_valid(
        self,
        target: MemoryBindingTarget,
        operation: MemoryBindingOperation,
    ) -> bool {
        // Intrinsically incompatible flag combination
        if self.contains(Self::PROCESS | Self::THREAD) {
            return false;
        }

        // Support for PROCESS and THREAD
        let good_for_target = match target {
            MemoryBindingTarget::Area => !self.intersects(Self::PROCESS | Self::THREAD),
            MemoryBindingTarget::Process => !self.contains(Self::THREAD),
            MemoryBindingTarget::None => true,
        };

        // Support fo STRICT, MIGRATE and NO_CPU_BINDING
        good_for_target
            && match operation {
                MemoryBindingOperation::GetLastLocation => {
                    !self.intersects(Self::STRICT | Self::MIGRATE | Self::NO_CPU_BINDING)
                }
                MemoryBindingOperation::GetBinding => {
                    if self.intersects(Self::MIGRATE | Self::NO_CPU_BINDING) {
                        return false;
                    }
                    match target {
                        MemoryBindingTarget::Area | MemoryBindingTarget::Process => true,
                        MemoryBindingTarget::None => {
                            !self.contains(Self::STRICT) || self.contains(Self::PROCESS)
                        }
                    }
                }
                MemoryBindingOperation::Unbind => !self.intersects(Self::STRICT | Self::MIGRATE),
                MemoryBindingOperation::Allocate => !self.contains(Self::MIGRATE),
                MemoryBindingOperation::Bind => true,
            }
    }
}
//
impl Default for MemoryBindingFlags {
    fn default() -> Self {
        Self::empty()
    }
}
//
/// Binding target
#[derive(Copy, Clone, Debug, Display, Eq, Hash, PartialEq)]
pub(crate) enum MemoryBindingTarget {
    Process,
    Area,
    None,
}
//
/// Binding operation
#[derive(Copy, Clone, Debug, Display, Eq, Hash, PartialEq)]
pub(crate) enum MemoryBindingOperation {
    GetBinding,
    Bind,
    Unbind,
    Allocate,
    GetLastLocation,
}

/// Rust mapping of the hwloc_membind_policy_t enum
///
/// We can't use Rust enums to model C enums in FFI because that results in
/// undefined behavior if the C API gets new enum variants and sends them to us.
///
pub(crate) type RawMemoryBindingPolicy = c_int;

/// Memory binding policy.
///
/// Not all systems support all kinds of binding.
/// [`Topology::support().memory_binding()`] may be used to query the
/// actual memory binding support in the currently used operating system.
#[repr(i32)]
#[derive(
    Copy, Clone, Debug, Default, Display, Eq, Hash, IntoPrimitive, PartialEq, TryFromPrimitive,
)]
#[doc(alias = "hwloc_membind_policy_t")]
pub enum MemoryBindingPolicy {
    /// Allocate each memory page individually on the local NUMA
    /// node of the thread that touches it
    ///
    /// The given nodeset should usually be [`Topology::nodeset()`]
    /// so that the touching thread may run and allocate on any node in the
    /// system.
    ///
    /// On AIX, if the nodeset is smaller, pages are allocated locally (if the
    /// local node is in the nodeset) or from a random non-local node (otherwise).
    #[doc(alias = "HWLOC_MEMBIND_FIRSTTOUCH")]
    FirstTouch = 1,

    /// Allocate memory on the specified nodes (most portable option)
    #[default]
    #[doc(alias = "HWLOC_MEMBIND_BIND")]
    Bind = 2,

    /// Allocate memory on the given nodes in an interleaved round-robin manner
    ///
    /// The precise layout of the memory across multiple NUMA nodes is OS/system
    /// specific.
    ///
    /// Interleaving can be useful when threads distributed across the specified
    /// NUMA nodes will all be accessing the whole memory range concurrently,
    /// since the interleave will then balance the memory references.
    #[doc(alias = "HWLOC_MEMBIND_INTERLEAVE")]
    Interleave = 3,

    /// Migrate pages on next touch
    ///
    /// For each page bound with this policy, by next time it is touched (and
    /// next time only), it is moved from its current location to the local NUMA
    /// node of the thread where the memory reference occurred (if it needs to
    /// be moved at all).
    #[doc(alias = "HWLOC_MEMBIND_NEXTTOUCH")]
    NextTouch = 4,
}

/// Errors that can occur when binding memory to NUMA nodes
#[derive(Copy, Clone, Debug, Error, Eq, Hash, PartialEq)]
pub enum MemoryBindingSetupError {
    /// Requested action or policy is not supported
    ///
    /// This error might not be reported if [`MemoryBindingFlags::STRICT`] is
    /// not set. Instead, the implementation is allowed to try to use a slightly
    /// different operation (with side-effects, smaller binding set, etc.) when
    /// the requested operation is not exactly supported.
    #[error("action is not supported")]
    Unsupported,

    /// Cannot bind to the target CPU or node set
    ///
    /// This error might not be reported if [`MemoryBindingFlags::STRICT`] is
    /// not set. Instead, the implementation is allowed to try using a smaller
    /// or larger set to make the operation succeed.
    #[error("binding cannot be enforced")]
    BadSet,

    /// Memory allocation failed even before trying to bind
    ///
    /// This error may only be returned by the `Topology::allocate_xyz` methods.
    #[error("memory allocation failed")]
    AllocationFailed,

    /// Unexpected errno value
    #[error("unexpected errno value {0}")]
    UnexpectedErrno(Errno),

    /// Unexpected binding function result
    #[error("unexpected binding function result {0} with errno {1}")]
    UnexpectedResult(i32, Errno),
}

impl MemoryBindingSetupError {
    /// Determine error from the current value of errno
    ///
    /// Call this after a memory binding function failed.
    pub(crate) fn from_errno() -> Self {
        let errno = errno();
        match errno.0 {
            ENOSYS => MemoryBindingSetupError::Unsupported,
            EXDEV => MemoryBindingSetupError::BadSet,
            ENOMEM => MemoryBindingSetupError::AllocationFailed,
            _ => MemoryBindingSetupError::UnexpectedErrno(errno),
        }
    }
}

/// Result of an operation that sets memory bindings
pub(crate) fn setup_result(result: i32) -> Result<(), MemoryBindingSetupError> {
    match result {
        x if x >= 0 => Ok(()),
        -1 => Err(MemoryBindingSetupError::from_errno()),
        negative => Err(MemoryBindingSetupError::UnexpectedResult(negative, errno())),
    }
}

/// Errors that can occur when querying current memory binding status
#[derive(Copy, Clone, Debug, Error, Eq, Hash, PartialEq)]
pub enum MemoryBindingQueryError {
    /// Memory policies and nodesets vary from one thread to another
    ///
    /// This error is returned when [`MemoryBindingFlags::PROCESS`] and
    /// [`MemoryBindingFlags::STRICT`] are both specified and the default memory
    /// policies and nodesets are not homogeneous across all threads of the
    /// target process.
    #[error("result varies from one thread of the process to another")]
    #[doc(alias = "HWLOC_MEMBIND_MIXED")]
    MixedResults,

    /// An invalid flag was specified
    ///
    /// Some memory binding flags like [`MemoryBindingFlags::MIGRATE`] do not
    /// make sense in the context of querying the current memory binding status
    /// and should not be used then.
    #[error("invalid request")]
    InvalidRequest,

    /// Unexpected errno value
    #[error("unexpected errno value {0}")]
    UnexpectedErrno(Errno),

    /// Unexpected binding function result
    #[error("unexpected binding function result {0} with errno {1}")]
    UnexpectedResult(i32, Errno),
}

/// Result of an operation that sets memory bindings
pub(crate) fn query_result_lazy<T>(
    result: i32,
    ok: impl FnOnce() -> T,
) -> Result<T, MemoryBindingQueryError> {
    match result {
        x if x >= 0 => Ok(ok()),
        -1 => Err({
            let errno = errno();
            match errno.0 {
                EXDEV => MemoryBindingQueryError::MixedResults,
                EINVAL => MemoryBindingQueryError::InvalidRequest,
                _ => MemoryBindingQueryError::UnexpectedErrno(errno),
            }
        }),
        negative => Err(MemoryBindingQueryError::UnexpectedResult(negative, errno())),
    }
}

/// Bytes allocated through hwloc
///
/// This behaves like a `Box<[MaybeUninit<u8>]>` and will similarly
/// automatically liberate the allocated memory when it goes out of scope.
pub struct Bytes<'topology> {
    /// Underlying hwloc topology
    topology: &'topology Topology,

    /// Previously allocated data pointer
    data: NonNull<[MaybeUninit<u8>]>,
}

impl<'topology> Bytes<'topology> {
    /// Wrap an hwloc allocation
    pub(crate) unsafe fn wrap(
        topology: &'topology Topology,
        base: *mut c_void,
        len: usize,
    ) -> Option<Self> {
        // Handle null pointers
        if base.is_null() {
            return None;
        }

        // Wrap the allocation
        let base = base as *mut MaybeUninit<u8>;
        let data = std::ptr::slice_from_raw_parts_mut(base, len);
        NonNull::new(data).map(|data| Self { topology, data })
    }
}

impl AsRef<[MaybeUninit<u8>]> for Bytes<'_> {
    fn as_ref(&self) -> &[MaybeUninit<u8>] {
        unsafe { self.data.as_ref() }
    }
}

impl AsMut<[MaybeUninit<u8>]> for Bytes<'_> {
    fn as_mut(&mut self) -> &mut [MaybeUninit<u8>] {
        unsafe { self.data.as_mut() }
    }
}

impl Borrow<[MaybeUninit<u8>]> for Bytes<'_> {
    fn borrow(&self) -> &[MaybeUninit<u8>] {
        self.as_ref()
    }
}

impl BorrowMut<[MaybeUninit<u8>]> for Bytes<'_> {
    fn borrow_mut(&mut self) -> &mut [MaybeUninit<u8>] {
        self.as_mut()
    }
}

impl Debug for Bytes<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Debug::fmt(self.as_ref(), f)
    }
}

impl Deref for Bytes<'_> {
    type Target = [MaybeUninit<u8>];

    fn deref(&self) -> &Self::Target {
        self.as_ref()
    }
}

impl DerefMut for Bytes<'_> {
    fn deref_mut(&mut self) -> &mut [MaybeUninit<u8>] {
        self.as_mut()
    }
}

impl Drop for Bytes<'_> {
    fn drop(&mut self) {
        let addr = self.data.as_ptr() as *mut MaybeUninit<u8> as *mut c_void;
        let len = self.data.len();
        let result = unsafe { ffi::hwloc_free(self.topology.as_ptr(), addr, len) };
        assert_eq!(
            result,
            0,
            "Got unexpected result from hwloc_free with errno {}",
            errno()
        );
    }
}