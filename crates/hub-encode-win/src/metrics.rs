use serde::Serialize;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};
use windows::{
    core::{w, PCWSTR},
    Win32::System::Performance::*,
};

#[derive(Serialize)]
pub struct GpuReport {
    pub status: String,
    pub samples: usize,
    pub video_encode_avg_percent: Option<f64>,
    pub video_encode_max_percent: Option<f64>,
    pub process_engine_sum_avg_percent: Option<f64>,
    pub per_engine_type_average_percent: BTreeMap<String, f64>,
}
pub struct GpuSampler {
    query: PDH_HQUERY,
    counter: PDH_HCOUNTER,
    last: Instant,
    status: String,
    encode: Vec<f64>,
    total: Vec<f64>,
    engines: BTreeMap<String, f64>,
}
impl Default for GpuSampler {
    fn default() -> Self {
        Self::new()
    }
}
impl GpuSampler {
    pub fn new() -> Self {
        let mut this = Self {
            query: PDH_HQUERY::default(),
            counter: PDH_HCOUNTER::default(),
            last: Instant::now(),
            status: "PDH process GPU Engine counters".into(),
            encode: Vec::new(),
            total: Vec::new(),
            engines: BTreeMap::new(),
        };
        unsafe {
            let opened = PdhOpenQueryW(PCWSTR::null(), 0, &mut this.query);
            if opened != 0 {
                this.status = format!("unavailable: PdhOpenQueryW {opened:#x}");
                return this;
            }
            let added = PdhAddEnglishCounterW(
                this.query,
                w!("\\GPU Engine(*)\\Utilization Percentage"),
                0,
                &mut this.counter,
            );
            if added != 0 {
                this.status = format!("unavailable: PdhAddEnglishCounterW {added:#x}");
                return this;
            }
            let collected = PdhCollectQueryData(this.query);
            if collected != 0 {
                this.status = format!("unavailable: initial PdhCollectQueryData {collected:#x}");
            }
        }
        this
    }
    pub fn sample(&mut self) {
        if self.status.starts_with("unavailable") || self.last.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.last = Instant::now();
        unsafe {
            let status = PdhCollectQueryData(self.query);
            if status != 0 {
                self.status = format!("unavailable: PdhCollectQueryData {status:#x}");
                return;
            }
            let mut bytes = 0;
            let mut count = 0;
            let status = PdhGetFormattedCounterArrayW(
                self.counter,
                PDH_FMT_DOUBLE,
                &mut bytes,
                &mut count,
                None,
            );
            if status != PDH_MORE_DATA || bytes > 4 * 1024 * 1024 {
                self.status = format!("unavailable: GPU array sizing {status:#x}");
                return;
            }
            // u64 storage provides the alignment required by PDH_FMT_COUNTERVALUE_ITEM_W.
            let mut storage = vec![0u64; (bytes as usize).div_ceil(8)];
            let ptr = storage.as_mut_ptr().cast::<PDH_FMT_COUNTERVALUE_ITEM_W>();
            let status = PdhGetFormattedCounterArrayW(
                self.counter,
                PDH_FMT_DOUBLE,
                &mut bytes,
                &mut count,
                Some(ptr),
            );
            if status != 0
                || count as usize
                    > storage.len() * 8 / std::mem::size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>()
            {
                self.status = format!("unavailable: GPU array {status:#x}");
                return;
            }
            let pid = format!("pid_{}_", std::process::id());
            let mut encode = 0.0;
            let mut total = 0.0;
            for item in std::slice::from_raw_parts(ptr, count as usize) {
                if item.FmtValue.CStatus > 1 {
                    continue;
                }
                let name = item.szName.to_string().unwrap_or_default();
                if name.starts_with(&pid) {
                    let value = item.FmtValue.Anonymous.doubleValue.max(0.0);
                    total += value;
                    let kind = name
                        .split_once("engtype_")
                        .map(|p| p.1)
                        .unwrap_or("unknown")
                        .to_owned();
                    *self.engines.entry(kind).or_default() += value;
                    if name.to_ascii_lowercase().contains("engtype_videoencode") {
                        encode += value;
                    }
                }
            }
            self.encode.push(encode);
            self.total.push(total);
        }
    }
    pub fn finish(self) -> GpuReport {
        let average = |data: &[f64]| {
            if data.is_empty() {
                None
            } else {
                Some(data.iter().sum::<f64>() / data.len() as f64)
            }
        };
        GpuReport {
            status: self.status.clone(),
            samples: self.encode.len(),
            video_encode_avg_percent: average(&self.encode),
            video_encode_max_percent: self.encode.iter().copied().reduce(f64::max),
            process_engine_sum_avg_percent: average(&self.total),
            per_engine_type_average_percent: self
                .engines
                .iter()
                .map(|(kind, value)| (kind.clone(), value / self.encode.len().max(1) as f64))
                .collect(),
        }
    }
}
impl Drop for GpuSampler {
    fn drop(&mut self) {
        if !self.query.is_invalid() {
            unsafe {
                PdhCloseQuery(self.query);
            }
        }
    }
}
