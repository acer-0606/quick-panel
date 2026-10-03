//! Resolve calendar filters in the current Windows timezone, including DST rules.
use crate::ai::Period;
use windows::Win32::{
    Foundation::{FILETIME, SYSTEMTIME},
    System::{
        SystemInformation::GetLocalTime,
        Time::{FileTimeToSystemTime, SystemTimeToFileTime, TzSpecificLocalTimeToSystemTimeEx},
    },
};

pub fn period_bounds(period: Period) -> Result<(Option<i64>, Option<i64>), String> {
    let offset = match period {
        Period::Any => return Ok((None, None)),
        Period::Today => 0,
        Period::Yesterday => -1,
        Period::Last7Days => -6,
        Period::Last30Days => -29,
    };
    unsafe {
        let mut midnight = GetLocalTime();
        midnight.wHour = 0;
        midnight.wMinute = 0;
        midnight.wSecond = 0;
        midnight.wMilliseconds = 0;
        let mut wall = FILETIME::default();
        SystemTimeToFileTime(&midnight, &mut wall).map_err(|_| "无法读取本地日期")?;
        let ticks = ((wall.dwHighDateTime as u64) << 32) | wall.dwLowDateTime as u64;
        let at = |days: i64| -> Result<i64, String> {
            let shifted: u64 = (ticks as i128 + days as i128 * 864_000_000_000)
                .try_into()
                .map_err(|_| "日期超出范围")?;
            let file = FILETIME {
                dwLowDateTime: shifted as u32,
                dwHighDateTime: (shifted >> 32) as u32,
            };
            let mut local = SYSTEMTIME::default();
            FileTimeToSystemTime(&file, &mut local).map_err(|_| "无法解析本地日期")?;
            let mut utc = SYSTEMTIME::default();
            TzSpecificLocalTimeToSystemTimeEx(None, &local, &mut utc)
                .map_err(|_| "无法转换本地时区")?;
            let mut timestamp = FILETIME::default();
            SystemTimeToFileTime(&utc, &mut timestamp).map_err(|_| "无法转换时间")?;
            let ticks = ((timestamp.dwHighDateTime as u64) << 32) | timestamp.dwLowDateTime as u64;
            let unix = ticks
                .checked_sub(116_444_736_000_000_000)
                .ok_or("日期早于支持范围")?
                / 10_000;
            i64::try_from(unix).map_err(|_| "日期超出范围".into())
        };
        let until = if period == Period::Yesterday { 0 } else { 1 };
        Ok((Some(at(offset)?), Some(at(until)?)))
    }
}
