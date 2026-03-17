use crate::sync::Mutex;
/// Transit pass system for Genesis wallet
///
/// Public transit cards, fare calculation,
/// route tracking, multi-city support.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum TransitType {
    Bus,
    Subway,
    Train,
    Tram,
    Ferry,
    BikeShare,
}

struct TransitPass {
    id: u32,
    city: [u8; 24],
    city_len: usize,
    balance_cents: u64,
    pass_type: TransitPassType,
    valid_until: Option<u64>,
    trip_count: u32,
}

#[derive(Clone, Copy, PartialEq)]
enum TransitPassType {
    PayPerRide,
    DailyPass,
    WeeklyPass,
    MonthlyPass,
    Annual,
}

struct TransitTrip {
    pass_id: u32,
    transit_type: TransitType,
    tap_in_time: u64,
    tap_out_time: Option<u64>,
    fare_cents: u64,
    station_in: [u8; 24],
    station_in_len: usize,
}

struct TransitEngine {
    passes: Vec<TransitPass>,
    trips: Vec<TransitTrip>,
    next_id: u32,
    active_trip: Option<usize>,
}

static TRANSIT: Mutex<Option<TransitEngine>> = Mutex::new(None);

impl TransitEngine {
    fn new() -> Self {
        TransitEngine {
            passes: Vec::new(),
            trips: Vec::new(),
            next_id: 1,
            active_trip: None,
        }
    }

    fn add_pass(&mut self, city: &[u8], pass_type: TransitPassType, balance: u64) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let mut c = [0u8; 24];
        let clen = city.len().min(24);
        c[..clen].copy_from_slice(&city[..clen]);
        self.passes.push(TransitPass {
            id,
            city: c,
            city_len: clen,
            balance_cents: balance,
            pass_type,
            valid_until: None,
            trip_count: 0,
        });
        id
    }

    fn tap_in(
        &mut self,
        pass_id: u32,
        transit_type: TransitType,
        station: &[u8],
        timestamp: u64,
    ) -> bool {
        if let Some(pass) = self.passes.iter().find(|p| p.id == pass_id) {
            if pass.balance_cents == 0 && pass.pass_type == TransitPassType::PayPerRide {
                return false;
            }
            let mut st = [0u8; 24];
            let slen = station.len().min(24);
            st[..slen].copy_from_slice(&station[..slen]);
            self.trips.push(TransitTrip {
                pass_id,
                transit_type,
                tap_in_time: timestamp,
                tap_out_time: None,
                fare_cents: 0,
                station_in: st,
                station_in_len: slen,
            });
            self.active_trip = Some(self.trips.len() - 1);
            return true;
        }
        false
    }

    fn tap_out(&mut self, timestamp: u64, fare_cents: u64) {
        if let Some(idx) = self.active_trip {
            if let Some(trip) = self.trips.get_mut(idx) {
                trip.tap_out_time = Some(timestamp);
                trip.fare_cents = fare_cents;
                let pid = trip.pass_id;
                if let Some(pass) = self.passes.iter_mut().find(|p| p.id == pid) {
                    pass.balance_cents = pass.balance_cents.saturating_sub(fare_cents);
                    pass.trip_count = pass.trip_count.saturating_add(1);
                }
            }
            self.active_trip = None;
        }
    }
}

pub fn init() {
    let mut t = TRANSIT.lock();
    *t = Some(TransitEngine::new());
    serial_println!("    Wallet: transit passes ready");
}
