//! CPU binding
//!
//! This module is all about checking and changing the binding of threads and
//! processes to hardware CPU cores.
//!
//! Most of this module's functionality is exposed via [methods of the Topology
//! struct](../../topology/struct.Topology.html#cpu-binding). The module itself
//! only hosts type definitions that are related to this functionality.

#[cfg(doc)]
use crate::{bitmap::Bitmap, object::types::ObjectType, topology::support::CpuBindingSupport};
use crate::{
    bitmap::RawBitmap,
    cpu::cpuset::CpuSet,
    errors::{self, FlagsError, HybridError, RawHwlocError},
    ffi,
    topology::{RawTopology, Topology},
    ProcessId, ThreadId,
};
use bitflags::bitflags;
use derive_more::Display;
use hwlocality_sys::{
    hwloc_cpubind_flags_t, HWLOC_CPUBIND_NOMEMBIND, HWLOC_CPUBIND_PROCESS, HWLOC_CPUBIND_STRICT,
    HWLOC_CPUBIND_THREAD,
};
use libc::{ENOSYS, EXDEV};
use std::{borrow::Borrow, ffi::c_int, fmt::Display};
use thiserror::Error;

/// # CPU binding
///
/// Some operating systems do not provide all hwloc-supported mechanisms to bind
/// processes, threads, etc. [`Topology::feature_support()`] may be used to
/// query about the actual CPU binding support in the currently used operating
/// system. The documentation of individual CPU binding functions will clarify
/// which support flags they require.
///
/// By default, when the requested binding operation is not available, hwloc
/// will go for a similar binding operation (with side-effects, smaller
/// binding set, etc). You can inhibit this with flag [`STRICT`], at the
/// expense of reducing portability across operating systems.
///
/// [`STRICT`]: CpuBindingFlags::STRICT
//
// Upstream docs: https://hwloc.readthedocs.io/en/v2.9/group__hwlocality__cpubinding.html
impl Topology {
    /// Binds the current process or thread on given CPUs
    ///
    /// Some operating systems only support binding threads or processes to a
    /// single [`PU`]. Others allow binding to larger sets such as entire
    /// [`Core`]s or [`Package`]s or even random sets of individual [`PU`]s. In
    /// such operating systems, the scheduler is free to run the task on one of
    /// these PU, then migrate it to another [`PU`], etc. It is often useful to
    /// call [`singlify()`] on the target CPU set before passing it to the
    /// binding function to avoid these expensive migrations.
    ///
    /// To unbind, just call the binding function with either a full cpuset or a
    /// cpuset equal to the system cpuset.
    ///
    /// You must specify exactly one of the [`ASSUME_SINGLE_THREAD`],
    /// [`THREAD`] and [`PROCESS`] binding target flags (listed in order of
    /// decreasing portability) when using this function.
    ///
    /// On some operating systems, CPU binding may have effects on memory
    /// binding, you can forbid this with flag [`NO_MEMORY_BINDING`].
    ///
    /// Running `lstopo --top` or `hwloc-ps` can be a very convenient tool to
    /// check how binding actually happened.
    ///
    /// Requires [`CpuBindingSupport::set_current_process()`] or
    /// [`CpuBindingSupport::set_current_thread()`] depending on flags.
    ///
    /// See also [the top-level CPU binding CPU
    /// documentation](../../topology/struct.Topology.html#cpu-binding).
    ///
    /// # Errors
    ///
    /// - [`BadObject(ThisProgram)`] if it is not possible to bind the current
    ///   process/thread to CPUs, generally speaking.
    /// - [`BadCpuSet`] if it is not possible to bind the current process/thread
    ///   to the requested CPU set, specifically.
    /// - [`BadFlags`] if the number of specified binding target flags is not
    ///   exactly one.
    ///
    /// [`ASSUME_SINGLE_THREAD`]: CpuBindingFlags::ASSUME_SINGLE_THREAD
    /// [`BadCpuSet`]: CpuBindingError::BadCpuSet
    /// [`BadFlags`]: CpuBindingError::BadFlags
    /// [`BadObject(ThisProgram)`]: CpuBindingError::BadObject
    /// [`Core`]: ObjectType::Core
    /// [`NO_MEMORY_BINDING`]: CpuBindingFlags::NO_MEMORY_BINDING
    /// [`Package`]: ObjectType::Package
    /// [`PROCESS`]: CpuBindingFlags::PROCESS
    /// [`PU`]: ObjectType::PU
    /// [`THREAD`]: CpuBindingFlags::THREAD
    /// [`singlify()`]: Bitmap::singlify()
    #[doc(alias = "hwloc_set_cpubind")]
    pub fn bind_cpu(
        &self,
        set: impl Borrow<CpuSet>,
        flags: CpuBindingFlags,
    ) -> Result<(), CpuBindingError> {
        let res = self.bind_cpu_impl(
            set.borrow(),
            flags,
            CpuBoundObject::ThisProgram,
            "hwloc_set_cpubind",
            |topology, cpuset, flags| unsafe { ffi::hwloc_set_cpubind(topology, cpuset, flags) },
        );
        match res {
            Ok(()) => Ok(()),
            Err(HybridError::Rust(e)) => Err(e),
            Err(HybridError::Hwloc(e)) => unreachable!("Unexpected hwloc error: {e}"),
        }
    }

    /// Get the current process or thread CPU binding
    ///
    /// You must specify exactly one of the [`ASSUME_SINGLE_THREAD`],
    /// [`THREAD`] and [`PROCESS`] binding target flags (listed in order of
    /// decreasing portability) when using this function.
    ///
    /// Flag [`NO_MEMORY_BINDING`] should not be used with this function.
    ///
    /// Requires [`CpuBindingSupport::get_current_process()`] or
    /// [`CpuBindingSupport::get_current_thread()`] depending on flags.
    ///
    /// See also [the top-level CPU binding CPU
    /// documentation](../../topology/struct.Topology.html#cpu-binding).
    ///
    /// # Errors
    ///
    /// - [`BadObject(ThisProgram)`] if it is not possible to query the CPU
    ///   binding of the current process/thread.
    /// - [`BadFlags`] if flag [`NO_MEMORY_BINDING`] was specified or if the
    ///   number of binding target flags is not exactly one.
    ///
    /// [`ASSUME_SINGLE_THREAD`]: CpuBindingFlags::ASSUME_SINGLE_THREAD
    /// [`BadFlags`]: CpuBindingError::BadFlags
    /// [`BadObject(ThisProgram)`]: CpuBindingError::BadObject
    /// [`NO_MEMORY_BINDING`]: CpuBindingFlags::NO_MEMORY_BINDING
    /// [`PROCESS`]: CpuBindingFlags::PROCESS
    /// [`THREAD`]: CpuBindingFlags::THREAD
    #[doc(alias = "hwloc_get_cpubind")]
    pub fn cpu_binding(
        &self,
        flags: CpuBindingFlags,
    ) -> Result<CpuSet, HybridError<CpuBindingError>> {
        self.cpu_binding_impl(
            flags,
            CpuBoundObject::ThisProgram,
            "hwloc_get_cpubind",
            |topology, cpuset, flags| unsafe { ffi::hwloc_get_cpubind(topology, cpuset, flags) },
        )
    }

    /// Binds a process (identified by its `pid`) on given CPUs
    ///
    /// As a special case on Linux, if a tid (thread ID) is supplied instead of
    /// a pid (process ID) and flag [`THREAD`] is specified, the specified
    /// thread is bound. Otherwise, flag [`THREAD`] should not be used with this
    /// function.
    ///
    /// See also [`Topology::bind_cpu()`] for more informations, except this
    /// requires [`CpuBindingSupport::set_process()`] or
    /// [`CpuBindingSupport::set_thread()`] depending on flags, and binding
    /// target flags other than [`THREAD`] should not be used with this
    /// function.
    ///
    /// # Errors
    ///
    /// - [`BadObject(ProcessOrThread)`] if it is not possible to bind the
    ///   target process/thread to CPUs, generally speaking.
    /// - [`BadCpuSet`] if it is not possible to bind the target process/thread
    ///   to the requested CPU set, specifically.
    /// - [`BadFlags`] if flag [`THREAD`] was specified on an operating system
    ///   other than Linux, or if any other binding target flag was specified.
    ///
    /// [`BadCpuSet`]: CpuBindingError::BadCpuSet
    /// [`BadFlags`]: CpuBindingError::BadFlags
    /// [`BadObject(ProcessOrThread)`]: CpuBindingError::BadObject
    /// [`THREAD`]: CpuBindingFlags::THREAD
    #[doc(alias = "hwloc_set_proc_cpubind")]
    pub fn bind_process_cpu(
        &self,
        pid: ProcessId,
        set: impl Borrow<CpuSet>,
        flags: CpuBindingFlags,
    ) -> Result<(), HybridError<CpuBindingError>> {
        self.bind_cpu_impl(
            set.borrow(),
            flags,
            CpuBoundObject::ProcessOrThread,
            "hwloc_set_proc_cpubind",
            |topology, cpuset, flags| unsafe {
                ffi::hwloc_set_proc_cpubind(topology, pid, cpuset, flags)
            },
        )
    }

    /// Get the current physical binding of a process, identified by its `pid`
    ///
    /// As a special case on Linux, if a tid (thread ID) is supplied instead of
    /// a pid (process ID) and flag [`THREAD`] is specified, the binding of the
    /// specified thread is returned. Otherwise, flag [`THREAD`] should not be
    /// used with this function.
    ///
    /// See [`Topology::cpu_binding()`] for more informations, except this
    /// requires [`CpuBindingSupport::get_process()`] or
    /// [`CpuBindingSupport::get_thread()`] depending on flags, and binding
    /// target flags other than [`THREAD`] should not be used with this
    /// function.
    ///
    /// # Errors
    ///
    /// - [`BadObject(ProcessOrThread)`] if it is not possible to query the CPU
    ///   binding of the target process/thread.
    /// - [`BadFlags`] if one of the  [`NO_MEMORY_BINDING`] was specified, if flag
    ///   [`THREAD`] was specified on an operating system other than Linux, or
    ///   if any other binding target flag was specified.
    ///
    /// [`BadFlags`]: CpuBindingError::BadFlags
    /// [`BadObject(ProcessOrThread)`]: CpuBindingError::BadObject
    /// [`NO_MEMORY_BINDING`]: CpuBindingFlags::NO_MEMORY_BINDING
    /// [`THREAD`]: CpuBindingFlags::THREAD
    #[doc(alias = "hwloc_get_proc_cpubind")]
    pub fn process_cpu_binding(
        &self,
        pid: ProcessId,
        flags: CpuBindingFlags,
    ) -> Result<CpuSet, HybridError<CpuBindingError>> {
        self.cpu_binding_impl(
            flags,
            CpuBoundObject::ProcessOrThread,
            "hwloc_get_proc_cpubind",
            |topology, cpuset, flags| unsafe {
                ffi::hwloc_get_proc_cpubind(topology, pid, cpuset, flags)
            },
        )
    }

    /// Bind a thread, identified by its `tid`, on the given CPUs
    ///
    /// See [`Topology::bind_cpu()`] for more informations, except this always
    /// requires [`CpuBindingSupport::set_thread()`] and binding target flags
    /// should not be used with this function.
    ///
    /// # Errors
    ///
    /// - [`BadObject(Thread)`] if it is not possible to bind the target thread
    ///   to CPUs, generally speaking.
    /// - [`BadCpuSet`] if it is not possible to bind the target thread to the
    ///   requested CPU set, specifically.
    /// - [`BadFlags`] if a binding target flag was specified.
    ///
    /// [`BadCpuSet`]: CpuBindingError::BadCpuSet
    /// [`BadFlags`]: CpuBindingError::BadFlags
    /// [`BadObject(Thread)`]: CpuBindingError::BadObject
    #[doc(alias = "hwloc_set_thread_cpubind")]
    pub fn bind_thread_cpu(
        &self,
        tid: ThreadId,
        set: impl Borrow<CpuSet>,
        flags: CpuBindingFlags,
    ) -> Result<(), HybridError<CpuBindingError>> {
        self.bind_cpu_impl(
            set.borrow(),
            flags,
            CpuBoundObject::Thread,
            "hwloc_set_thread_cpubind",
            |topology, cpuset, flags| unsafe {
                ffi::hwloc_set_thread_cpubind(topology, tid, cpuset, flags)
            },
        )
    }

    /// Get the current physical binding of thread `tid`
    ///
    /// Flags [`STRICT`], [`NO_MEMORY_BINDING`] and binding target flags should
    /// not be used with this function.
    ///
    /// See [`Topology::cpu_binding()`] for more informations, except this
    /// requires [`CpuBindingSupport::get_thread()`] and binding target flags
    /// should not be used with this function.
    ///
    /// # Errors
    ///
    /// - [`BadObject(Thread)`] if it is not possible to query the CPU
    ///   binding of the target thread.
    /// - [`BadFlags`] if at least one of flags [`STRICT`] and
    ///   [`NO_MEMORY_BINDING`] or a binding target flag was specified.
    ///
    /// [`ASSUME_SINGLE_THREAD`]: CpuBindingFlags::ASSUME_SINGLE_THREAD
    /// [`BadFlags`]: CpuBindingError::BadFlags
    /// [`BadObject(Thread)`]: CpuBindingError::BadObject
    /// [`NO_MEMORY_BINDING`]: CpuBindingFlags::NO_MEMORY_BINDING
    /// [`PROCESS`]: CpuBindingFlags::PROCESS
    /// [`STRICT`]: CpuBindingFlags::STRICT
    /// [`THREAD`]: CpuBindingFlags::THREAD
    #[doc(alias = "hwloc_get_thread_cpubind")]
    pub fn thread_cpu_binding(
        &self,
        tid: ThreadId,
        flags: CpuBindingFlags,
    ) -> Result<CpuSet, HybridError<CpuBindingError>> {
        self.cpu_binding_impl(
            flags,
            CpuBoundObject::Thread,
            "hwloc_get_thread_cpubind",
            |topology, cpuset, flags| unsafe {
                ffi::hwloc_get_thread_cpubind(topology, tid, cpuset, flags)
            },
        )
    }

    /// Get the last physical CPUs where the current process or thread ran
    ///
    /// The operating system may move some tasks from one processor
    /// to another at any time according to their binding,
    /// so this function may return something that is already
    /// outdated.
    ///
    /// You must specify exactly one of the [`ASSUME_SINGLE_THREAD`],
    /// [`THREAD`] and [`PROCESS`] binding target flags (listed in order of
    /// decreasing portability) when using this function.
    ///
    /// Flags [`NO_MEMORY_BINDING`] and [`STRICT`] should not be used with this
    /// function.
    ///
    /// Requires [`CpuBindingSupport::get_current_process_last_cpu_location()`]
    /// or [`CpuBindingSupport::get_current_thread_last_cpu_location()`]
    /// depending on flags.
    ///
    /// See also [the top-level CPU binding CPU
    /// documentation](../../topology/struct.Topology.html#cpu-binding).
    ///
    /// # Errors
    ///
    /// - [`BadObject(ThisProgram)`] if it is not possible to query the CPU
    ///   location of the current process/thread.
    /// - [`BadFlags`] if one of flags [`NO_MEMORY_BINDING`] and [`STRICT`] was
    ///   specified, or if the number of binding target flags is not exactly
    ///   one.
    ///
    /// [`ASSUME_SINGLE_THREAD`]: CpuBindingFlags::ASSUME_SINGLE_THREAD
    /// [`BadFlags`]: CpuBindingError::BadFlags
    /// [`BadObject(ThisProgram)`]: CpuBindingError::BadObject
    /// [`NO_MEMORY_BINDING`]: CpuBindingFlags::NO_MEMORY_BINDING
    /// [`PROCESS`]: CpuBindingFlags::PROCESS
    /// [`STRICT`]: CpuBindingFlags::STRICT
    /// [`THREAD`]: CpuBindingFlags::THREAD
    #[doc(alias = "hwloc_get_last_cpu_location")]
    pub fn last_cpu_location(
        &self,
        flags: CpuBindingFlags,
    ) -> Result<CpuSet, HybridError<CpuBindingError>> {
        self.last_cpu_location_impl(
            flags,
            CpuBoundObject::ThisProgram,
            "hwloc_get_last_cpu_location",
            |topology, cpuset, flags| unsafe {
                ffi::hwloc_get_last_cpu_location(topology, cpuset, flags)
            },
        )
    }

    /// Get the last physical CPU where a process ran.
    ///
    /// As a special case on Linux, if a tid (thread ID) is supplied instead of
    /// a pid (process ID) and flag [`THREAD`] is specified, the last cpu
    /// location of the specified thread is returned. Otherwise, flag [`THREAD`]
    /// should not be used with this function.
    ///
    /// See [`Topology::last_cpu_location()`] for more informations, except this
    /// requires [`CpuBindingSupport::get_process_last_cpu_location()`], and
    /// binding target flags other than [`THREAD`] should not be used with this
    /// function.
    ///
    /// # Errors
    ///
    /// - [`BadObject(ProcessOrThread)`] if it is not possible to query the CPU
    ///   binding of the target process/thread.
    /// - [`BadFlags`] if one of flags [`NO_MEMORY_BINDING`] and [`STRICT`] was
    ///   specified, if flag[`THREAD`] was specified on an operating system
    ///   other than Linux, or if any other binding target flag was specified.
    ///
    /// [`BadFlags`]: CpuBindingError::BadFlags
    /// [`BadObject(ProcessOrThread)`]: CpuBindingError::BadObject
    /// [`NO_MEMORY_BINDING`]: CpuBindingFlags::NO_MEMORY_BINDING
    /// [`STRICT`]: CpuBindingFlags::STRICT
    /// [`THREAD`]: CpuBindingFlags::THREAD
    #[doc(alias = "hwloc_get_proc_last_cpu_location")]
    pub fn last_process_cpu_location(
        &self,
        pid: ProcessId,
        flags: CpuBindingFlags,
    ) -> Result<CpuSet, HybridError<CpuBindingError>> {
        self.last_cpu_location_impl(
            flags,
            CpuBoundObject::ProcessOrThread,
            "hwloc_get_proc_last_cpu_location",
            |topology, cpuset, flags| unsafe {
                ffi::hwloc_get_proc_last_cpu_location(topology, pid, cpuset, flags)
            },
        )
    }

    /// Binding for set_cpubind style functions
    fn bind_cpu_impl(
        &self,
        set: &CpuSet,
        flags: CpuBindingFlags,
        target: CpuBoundObject,
        api: &'static str,
        ffi: impl FnOnce(*const RawTopology, *const RawBitmap, c_int) -> c_int,
    ) -> Result<(), HybridError<CpuBindingError>> {
        let Some(flags) = flags.validate(target, CpuBindingOperation::SetBinding) else {
            return Err(CpuBindingError::from(flags).into());
        };
        call_hwloc(api, target, Some(set), || {
            ffi(
                self.as_ptr(),
                set.as_ptr(),
                i32::try_from(flags.bits()).expect("Unexpected high order bit in flags"),
            )
        })
    }

    /// Binding for get_cpubind style functions
    fn cpu_binding_impl(
        &self,
        flags: CpuBindingFlags,
        target: CpuBoundObject,
        api: &'static str,
        ffi: impl FnOnce(*const RawTopology, *mut RawBitmap, c_int) -> c_int,
    ) -> Result<CpuSet, HybridError<CpuBindingError>> {
        self.get_cpuset(flags, target, CpuBindingOperation::GetBinding, api, ffi)
    }

    /// Binding for get_last_cpu_location style functions
    fn last_cpu_location_impl(
        &self,
        flags: CpuBindingFlags,
        target: CpuBoundObject,
        api: &'static str,
        ffi: impl FnOnce(*const RawTopology, *mut RawBitmap, c_int) -> c_int,
    ) -> Result<CpuSet, HybridError<CpuBindingError>> {
        self.get_cpuset(
            flags,
            target,
            CpuBindingOperation::GetLastLocation,
            api,
            ffi,
        )
    }

    /// Binding for all functions that get CPU bindings
    fn get_cpuset(
        &self,
        flags: CpuBindingFlags,
        target: CpuBoundObject,
        operation: CpuBindingOperation,
        api: &'static str,
        ffi: impl FnOnce(*const RawTopology, *mut RawBitmap, c_int) -> c_int,
    ) -> Result<CpuSet, HybridError<CpuBindingError>> {
        let Some(flags) = flags.validate(target, operation) else {
            return Err(CpuBindingError::from(flags).into());
        };
        let mut cpuset = CpuSet::new();
        call_hwloc(api, target, None, || {
            ffi(
                self.as_ptr(),
                cpuset.as_mut_ptr(),
                i32::try_from(flags.bits()).expect("Unexpected high order bit in flags"),
            )
        })
        .map(|()| cpuset)
    }
}

bitflags! {
    /// Process/Thread binding flags
    ///
    /// These bit flags can be used to refine the binding policy. All flags can
    /// be OR'ed together with the exception of the binding targets flags
    /// `ASSUME_SINGLE_THREAD`, `THREAD` and `PROCESS`, which are mutually
    /// exclusive.
    ///
    /// When using one of the functions that target the active process, you must
    /// use exactly one of these flags. The most portable binding targets are
    /// `ASSUME_SINGLE_THREAD`, `THREAD` and `PROCESS`, in this order. These
    /// flags must generally not be used with any other function, except on
    /// Linux where flag `THREAD` can also be used to turn process-binding
    /// functions into thread-binding functions.
    ///
    /// Individual CPU binding functions may not support all of these flags.
    /// Please check the documentation of the `Topology` method that you are
    /// trying to call for more information.
    #[derive(Copy, Clone, Debug, Eq, Hash, PartialEq)]
    #[doc(alias = "hwloc_cpubind_flags_t")]
    pub struct CpuBindingFlags: hwloc_cpubind_flags_t {
        /// Assume that the current process is single threaded
        ///
        /// This lets hwloc pick between thread and process binding for
        /// increased portability.
        ///
        /// This is mutually exclusive with `PROCESS` and `THREAD`.
        //
        // NOTE: This is not an actual hwloc flag, and must be cleared before
        //       invoking hwloc. Please let validate() do this for you.
        const ASSUME_SINGLE_THREAD = (1<<31);

        /// Bind the current thread of the current process
        ///
        /// This is the second most portable option where `ASSUME_SINGLE_THREAD`
        /// is inapplicable.
        ///
        /// On Linux, this flag can also be used to turn process-binding
        /// functions into thread-binding functions.
        ///
        /// This is mutually exclusive with `ASSUME_SINGLE_THREAD` and `PROCESS`.
        #[doc(alias = "HWLOC_CPUBIND_THREAD")]
        const THREAD  = HWLOC_CPUBIND_THREAD;

        /// Bind all threads of the current process
        ///
        /// This is mutually exclusive with `ASSUME_SINGLE_THREAD` and `THREAD`.
        #[doc(alias = "HWLOC_CPUBIND_PROCESS")]
        const PROCESS = HWLOC_CPUBIND_PROCESS;

        /// Request for strict binding from the OS
        ///
        /// By default, when the designated CPUs are all busy while other CPUs
        /// are idle, operating systems may execute the thread/process on those
        /// other CPUs instead of the designated CPUs, to let them progress
        /// anyway. Strict binding means that the thread/process will _never_
        /// execute on other CPUs than the designated CPUs, even when those are
        /// busy with other tasks and other CPUs are idle.
        ///
        /// Depending on the operating system, strict binding may not be
        /// possible (e.g. the OS does not implement it) or not allowed (e.g.
        /// for an administrative reasons), and the binding function will fail
        /// in that case.
        ///
        /// When retrieving the binding of a process, this flag checks whether
        /// all its threads actually have the same binding. If the flag is not
        /// given, the binding of each thread will be accumulated.
        ///
        /// This flag should not be used when retrieving the binding of a
        /// thread or the CPU location of a process.
        #[doc(alias = "HWLOC_CPUBIND_STRICT")]
        const STRICT = HWLOC_CPUBIND_STRICT;

        /// Avoid any effect on memory binding
        ///
        /// On some operating systems, some CPU binding function would also bind
        /// the memory on the corresponding NUMA node. It is often not a
        /// problem for the application, but if it is, setting this flag will
        /// make hwloc avoid using OS functions that would also bind memory.
        /// This will however reduce the support of CPU bindings, i.e.
        /// potentially result in the binding function erroring out with a
        /// [`CpuBindingError`].
        ///
        /// This flag should only be used with functions that set the CPU
        /// binding.
        #[doc(alias = "HWLOC_CPUBIND_NOMEMBIND")]
        const NO_MEMORY_BINDING = HWLOC_CPUBIND_NOMEMBIND;
    }
}
//
// NOTE: No Default because user must consciously think about the need for
//       PROCESS vs ASSUME_SINGLE_THREAD.
//
impl CpuBindingFlags {
    /// Check that these flags are in a valid state, emit validated flags free
    /// of ASSUME_SINGLE_THREAD and ready for hwloc consumption.
    pub(crate) fn validate(
        mut self,
        target: CpuBoundObject,
        operation: CpuBindingOperation,
    ) -> Option<Self> {
        // THREAD can only be specified on process binding functions on Linux,
        // to turn them into thread binding functions.
        let is_linux_thread_special_case =
            self.contains(Self::THREAD) && target == CpuBoundObject::ProcessOrThread;
        if is_linux_thread_special_case && cfg!(not(target_os = "linux")) {
            return None;
        }

        // Must use exactly one target flag when targeting the active process,
        // and none otherwise, except for the special case discussed above.
        let num_target_flags = (self & (Self::PROCESS | Self::THREAD | Self::ASSUME_SINGLE_THREAD))
            .bits()
            .count_ones();
        if (num_target_flags != (target == CpuBoundObject::ThisProgram) as u32)
            && !(num_target_flags == 1 && is_linux_thread_special_case)
        {
            return None;
        }

        // Operation-specific considerations
        match operation {
            CpuBindingOperation::GetLastLocation => {
                if self.intersects(Self::STRICT | Self::NO_MEMORY_BINDING) {
                    return None;
                }
            }
            CpuBindingOperation::SetBinding => {}
            CpuBindingOperation::GetBinding => {
                if (self.contains(Self::STRICT) && target == CpuBoundObject::Thread)
                    || self.contains(Self::NO_MEMORY_BINDING)
                {
                    return None;
                }
            }
        }

        // Clear virtual ASSUME_SINGLE_THREAD flag, which served its purpose
        self.remove(CpuBindingFlags::ASSUME_SINGLE_THREAD);
        Some(self)
    }
}
//
/// Object that is being bound to particular CPUs
#[derive(Copy, Clone, Debug, Eq, Hash, PartialEq)]
pub enum CpuBoundObject {
    /// A process, identified by its PID, or possibly a thread on Linux
    ProcessOrThread,

    /// A thread, identified by its TID
    Thread,

    /// The currently running program
    ThisProgram,
}
//
impl Display for CpuBoundObject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let display = match self {
            Self::ProcessOrThread => "the target process/thread",
            Self::Thread => "the target thread",
            Self::ThisProgram => "the current process/thread",
        };
        f.pad(display)
    }
}
//
/// Operation on that object's CPU binding
#[derive(Copy, Clone, Debug, Display, Eq, Hash, PartialEq)]
pub(crate) enum CpuBindingOperation {
    /// `hwloc_get_cpubind()`-like operation
    GetBinding,

    /// `hwloc_set_cpubind()`-like operation
    SetBinding,

    /// `hwloc_get_last_cpu_location()`-like operation
    GetLastLocation,
}

/// Errors that can occur when binding processes or threads to CPUSets
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum CpuBindingError {
    /// Cannot query or set CPU bindings for this kind of object
    ///
    /// This error might not be reported if [`CpuBindingFlags::STRICT`] is not
    /// set. Instead, the implementation is allowed to try to use a slightly
    /// different operation (with side-effects, larger object, etc.) when the
    /// requested operation is not exactly supported.
    #[error("cannot query or set the CPU binding of {0}")]
    BadObject(CpuBoundObject),

    /// Requested CPU binding flags are not valid in this context
    ///
    /// Not all CPU binding flag combinations make sense, either in isolation or
    /// in the context of a particular binding function. Please cross-check the
    /// documentation of [`CpuBindingFlags`] as well as that of the function
    /// you were trying to call for more information.
    #[error(transparent)]
    BadFlags(#[from] FlagsError<CpuBindingFlags>),

    /// Cannot bind the requested object to the target cpu set
    ///
    /// Operating systems can have various restrictions here, e.g. can only bind
    /// to one CPU, one NUMA node, etc.
    ///
    /// This error should only be reported when trying to set CPU bindings.
    ///
    /// This error might not be reported if [`CpuBindingFlags::STRICT`] is not
    /// set. Instead, the implementation is allowed to try to use a slightly
    /// different operation (with side-effects, smaller binding set, etc.) when
    /// the requested operation is not exactly supported.
    #[error("cannot bind {0} to {1}")]
    BadCpuSet(CpuBoundObject, CpuSet),
}
//
impl From<CpuBindingFlags> for CpuBindingError {
    fn from(value: CpuBindingFlags) -> Self {
        Self::BadFlags(value.into())
    }
}

/// Call an hwloc API that is about getting or setting CPU bindings, translate
/// known errors into higher-level `CpuBindingError`s.
///
/// Validating flags is left up to the caller, to avoid allocating result
/// objects when it can be proved upfront that the request is invalid.
pub(crate) fn call_hwloc(
    api: &'static str,
    object: CpuBoundObject,
    cpuset: Option<&CpuSet>,
    ffi: impl FnOnce() -> c_int,
) -> Result<(), HybridError<CpuBindingError>> {
    match errors::call_hwloc_int_normal(api, ffi) {
        Ok(_positive) => Ok(()),
        Err(
            raw_err @ RawHwlocError {
                errno: Some(errno), ..
            },
        ) => match errno.0 {
            ENOSYS => Err(CpuBindingError::BadObject(object).into()),
            EXDEV => Err(CpuBindingError::BadCpuSet(
                object,
                cpuset
                    .expect("This error should only be observed on commands that bind to CPUs")
                    .clone(),
            )
            .into()),
            _ => Err(HybridError::Hwloc(raw_err)),
        },
        Err(raw_err) => Err(HybridError::Hwloc(raw_err)),
    }
}
