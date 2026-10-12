//! The application time zone, like Laravel's `app.timezone`: the zone of
//! the date-times the ORM writes (timestamps, soft deletes, pivot
//! timestamps) and the zone `DATETIME` / `TIMESTAMP` columns are read in.

use std::sync::{Arc, RwLock};

use chrono::{DateTime, NaiveDateTime, Offset, TimeZone, Utc};

/// A [`TimeZone`] with its offset type erased, so any zone fits one static.
trait Zone: Send + Sync {
    fn local(&self, utc: &DateTime<Utc>) -> NaiveDateTime;
    fn utc(&self, local: &NaiveDateTime) -> DateTime<Utc>;
}

impl<Tz: TimeZone + Send + Sync> Zone for Tz {
    fn local(&self, utc: &DateTime<Utc>) -> NaiveDateTime {
        utc.with_timezone(self).naive_local()
    }

    fn utc(&self, local: &NaiveDateTime) -> DateTime<Utc> {
        match self.from_local_datetime(local).earliest() {
            Some(time) => time.with_timezone(&Utc),
            // A wall time skipped by a daylight-saving jump: use the offset
            // in force around it, as Carbon does.
            None => (*local - self.offset_from_utc_datetime(local).fix()).and_utc(),
        }
    }
}

static ZONE: RwLock<Option<Arc<dyn Zone>>> = RwLock::new(None);

/// Sets the application time zone; UTC until called. Like Laravel's
/// `app.timezone`, it is process-wide, so set it once at startup.
///
/// Date-times stored without a zone (MySQL `DATETIME`, Oracle `TIMESTAMP`)
/// then hold wall-clock time in `zone`:
///
/// - `created_at`, `updated_at`, `deleted_at`, `touch` and pivot timestamps
///   are filled with [`now`] in `zone`;
/// - a [`DateTime<Utc>`] is written as its wall-clock time in `zone`, and
///   read back by converting from it; a `NaiveDateTime` is never converted;
/// - Oracle `TIMESTAMP WITH TIME ZONE` columns read as wall-clock time in `zone`.
///
/// Any `chrono` time zone works: [`chrono::FixedOffset`], [`chrono::Local`]
/// (the system zone), or a named zone such as
/// `chrono_tz::Europe::Bratislava` from the `chrono-tz` crate, which follows
/// daylight saving time.
///
/// ```
/// rustique::set_timezone(chrono::FixedOffset::east_opt(2 * 3600).unwrap());
/// # rustique::set_timezone(chrono::Utc);
/// ```
///
/// Schema defaults from [`use_current`](crate::Column::use_current) stay UTC.
///
/// # Panics
///
/// When another thread panicked while setting the zone.
pub fn set_timezone<Tz: TimeZone + Send + Sync + 'static>(zone: Tz) {
    *ZONE.write().expect("time zone lock poisoned") = Some(Arc::new(zone));
}

fn zone() -> Option<Arc<dyn Zone>> {
    ZONE.read().expect("time zone lock poisoned").clone()
}

/// The current wall-clock time in the application time zone (see
/// [`set_timezone`]), like Laravel's `now()`.
#[must_use]
pub fn now() -> NaiveDateTime {
    local(&Utc::now())
}

/// `utc` as wall-clock time in the application time zone.
pub(crate) fn local(utc: &DateTime<Utc>) -> NaiveDateTime {
    zone().map_or_else(|| utc.naive_utc(), |zone| zone.local(utc))
}

/// Wall-clock time in the application time zone, as UTC.
pub(crate) fn utc(local: &NaiveDateTime) -> DateTime<Utc> {
    zone().map_or_else(|| local.and_utc(), |zone| zone.utc(local))
}
