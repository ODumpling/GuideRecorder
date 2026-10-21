//! Process CPU time, for a recording's [`SessionMetrics`]: `getrusage` on
//! macOS, `GetProcessTimes` on Windows.
//!
//! [`SessionMetrics`]: crate::backend::session::SessionMetrics

use std::time::Duration;

/// CPU time the whole process has used, all threads.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CpuTime {
    pub user: Duration,
    pub system: Duration,
}

#[cfg(target_os = "macos")]
pub(crate) use darwin::process_cpu_time;
#[cfg(windows)]
pub(crate) use win::process_cpu_time;

#[cfg(target_os = "macos")]
mod darwin {
    use std::time::Duration;

    use super::CpuTime;

    #[repr(C)]
    #[derive(Default)]
    struct Timeval {
        tv_sec: i64,
        /// `suseconds_t` is `i32` on Darwin.
        tv_usec: i32,
    }

    /// Darwin `struct rusage`: two timevals and 14 longs.
    #[repr(C)]
    #[derive(Default)]
    struct Rusage {
        ru_utime: Timeval,
        ru_stime: Timeval,
        rest: [i64; 14],
    }

    const _: () = assert!(std::mem::size_of::<Rusage>() == 144);

    const RUSAGE_SELF: i32 = 0;

    extern "C" {
        fn getrusage(who: i32, usage: *mut Rusage) -> i32;
    }

    pub(crate) fn process_cpu_time() -> Option<CpuTime> {
        let mut usage = Rusage::default();
        // SAFETY: `usage` is a valid, correctly sized out-pointer.
        if unsafe { getrusage(RUSAGE_SELF, &mut usage) } != 0 {
            return None;
        }
        let dur = |t: &Timeval| {
            Duration::from_secs(t.tv_sec.max(0) as u64)
                + Duration::from_micros(t.tv_usec.max(0) as u64)
        };
        Some(CpuTime {
            user: dur(&usage.ru_utime),
            system: dur(&usage.ru_stime),
        })
    }
}

#[cfg(windows)]
mod win {
    use std::time::Duration;

    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

    use super::CpuTime;

    pub(crate) fn process_cpu_time() -> Option<CpuTime> {
        let mut times = [FILETIME::default(); 4];
        let [created, exited, kernel, user] = &mut times;
        // SAFETY: the pseudo handle of this process and four valid
        // out-pointers.
        unsafe { GetProcessTimes(GetCurrentProcess(), created, exited, kernel, user) }.ok()?;
        // 100 ns units.
        let dur = |t: &FILETIME| {
            Duration::from_nanos(
                ((u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime))
                    .saturating_mul(100),
            )
        };
        Some(CpuTime {
            user: dur(user),
            system: dur(kernel),
        })
    }
}
