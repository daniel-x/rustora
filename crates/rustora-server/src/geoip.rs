// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `GeoIP.cs`: the country of an IP address from an IP2Location LITE DB1 database.

use std::io::Read;
use std::net::IpAddr;
use std::path::Path;

/// The file name of the database, which `fetch-geoip.sh` of OpenRA downloads into the engine directory.
pub const DATABASE_FILE: &str = "IP2LOCATION-LITE-DB1.IPV6.BIN.ZIP";
const DATABASE_ENTRY: &str = "IP2LOCATION-LITE-DB1.IPV6.BIN";

/// Port of `IP2LocationReader`.
pub struct GeoIp {
    data: Vec<u8>,
    column_count: u64,
    v4_count: u64,
    v4_offset: u64,
    v6_count: u64,
    v6_offset: u64,
}

impl GeoIp {
    /// `GeoIP.Initialize`: the database in the zip file `path`, `None` when the file is missing or unreadable.
    pub fn load(path: &Path) -> Option<GeoIp> {
        if !path.is_file() {
            return None;
        }
        match Self::read_zip(path).and_then(Self::parse) {
            Ok(db) => Some(db),
            Err(e) => {
                log::info!(target: "geoip", "DatabaseReader failed: {e}");
                None
            }
        }
    }

    fn read_zip(path: &Path) -> Result<Vec<u8>, String> {
        let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
        let mut zip = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
        let mut entry = zip.by_name(DATABASE_ENTRY).map_err(|e| e.to_string())?;
        let mut data = Vec::new();
        entry.read_to_end(&mut data).map_err(|e| e.to_string())?;
        Ok(data)
    }

    fn parse(data: Vec<u8>) -> Result<GeoIp, String> {
        if data.len() < 21 {
            return Err("truncated database header".to_string());
        }
        if data[0] != 1 {
            return Err("Only IP2Location type 1 databases are supported.".to_string());
        }
        let u32_at = |i: usize| u64::from(u32::from_le_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]));
        Ok(GeoIp {
            column_count: u64::from(data[1]),
            v4_count: u32_at(5),
            v4_offset: u32_at(9),
            v6_count: u32_at(13),
            v6_offset: u32_at(17),
            data,
        })
    }

    fn bytes(&self, pos: u64, len: usize) -> Result<&[u8], String> {
        let start = usize::try_from(pos).map_err(|e| e.to_string())?;
        self.data.get(start..start + len).ok_or_else(|| format!("read past the end at {pos}"))
    }

    fn row(&self, index: u64, v6: bool) -> u64 {
        let (start, offset) = if v6 { (self.v6_offset, 12) } else { (self.v4_offset, 0) };
        start + index * (4 * self.column_count + offset)
    }

    fn address_for_index(&self, index: u64, v6: bool) -> Result<u128, String> {
        let bytes = self.bytes(self.row(index, v6) - 1, if v6 { 16 } else { 4 })?;
        Ok(bytes.iter().rev().fold(0u128, |n, &b| (n << 8) | u128::from(b)))
    }

    fn country_for_index(&self, index: u64, v6: bool) -> Result<Option<String>, String> {
        let offset = if v6 { 12 } else { 0 };
        let b = self.bytes(self.row(index, v6) + offset + 3, 4)?;
        let country_offset = u64::from(u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
        let length = usize::from(self.bytes(country_offset + 3, 1)?[0]);
        let country = String::from_utf8_lossy(self.bytes(country_offset + 4, length)?).into_owned();
        Ok((country != "-").then_some(country))
    }

    fn try_lookup(&self, ip: IpAddr) -> Result<Option<String>, String> {
        let (number, v6, count) = match ip {
            IpAddr::V4(a) => (u128::from(u32::from(a)), false, self.v4_count),
            IpAddr::V6(a) => (u128::from(a), true, self.v6_count),
        };
        let (mut low, mut high) = (0i64, count as i64);
        while low <= high {
            let mid = (low + high) / 2;
            let min = self.address_for_index(mid as u64, v6)?;
            let max = self.address_for_index(mid as u64 + 1, v6)?;
            if min <= number && number < max {
                return self.country_for_index(mid as u64, v6);
            }
            if number < min {
                high = mid - 1;
            } else {
                low = mid + 1;
            }
        }
        Ok(None)
    }

    /// `GeoIP.LookupCountry`: the country name of `ip`, `None` for unknown and private addresses.
    pub fn lookup_country(&self, ip: IpAddr) -> Option<String> {
        self.try_lookup(ip).unwrap_or_else(|e| {
            log::info!(target: "geoip", "LookupCountry failed: {e}");
            None
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn database() -> Option<GeoIp> {
        GeoIp::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../openra-ref").join(DATABASE_FILE))
    }

    #[test]
    fn looks_up_countries() {
        let Some(db) = database() else { return };
        assert_eq!(db.lookup_country("8.8.8.8".parse().unwrap()).as_deref(), Some("United States of America"));
        assert_eq!(db.lookup_country("127.0.0.1".parse().unwrap()), None);
        assert_eq!(db.lookup_country("2001:4860:4860::8888".parse().unwrap()).as_deref(), Some("United States of America"));
        assert_eq!(db.lookup_country("::ffff:8.8.8.8".parse().unwrap()), None);
    }
}
