# ZIB0024 Search timestamps with calendar values

## Context

Timestamp fields store integer milliseconds since the Unix epoch. The search box currently requires
that representation, which makes fields such as `captured-at` hard to search. The owner requests
partial timestamps that match the whole period at their supplied precision, interpreted in the local
timezone to match the displayed capture time.

## Decision

Accept a year, a year and month, or a complete date followed by optional hours, minutes, seconds,
and milliseconds. Interpret a partial timestamp as the period at its supplied precision. Equality
matches the whole period. A range starts at the first millisecond of its first period. An inclusive
end includes the final millisecond of its last period; an exclusive end stops at that period's first
millisecond.

Use a period's start for `<` and `>=`, and its end for `<=` and `>`. Use the machine's local
timezone unless the value supplies `Z` or a numeric offset. Keep integer millisecond inputs and
timestamps with three fractional digits exact, without changing the stored field value.

Give four unsigned digits the calendar-year interpretation. Other plain integers keep their epoch
millisecond interpretation. Add an `ms:` prefix for exact millisecond inputs, including values that
would otherwise look like years. This makes the owner's `2026` example work without requiring an
extra marker on ordinary calendar searches.

Use `chrono` in the core for calendar validation and timezone conversion. It handles changes in the
local offset for the date under comparison. Reject local times that occur twice or don't exist at
clock changes, and ask for an explicit offset. Don't guess which instant the owner means.

Resolve each boundary in its own calendar context. A local day can span 23 or 25 hours when clocks
change. Don't add a fixed number of milliseconds to find its last instant. The
[search ranges specification][ranges] defines the inputs.

## Rejected alternatives

- **Fill omitted components with zero for every comparison.** This makes a date-only equality match
  midnight alone and requires two dates to search one day. The owner requests matching the whole
  supplied period instead.

## Consequences

A saved query without an explicit offset follows the timezone of the machine that runs it. A query
with an explicit offset denotes the same instants on every machine.

[ranges]: ../specs/search-ranges.md
