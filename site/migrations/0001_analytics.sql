-- Website analytics stays separate from Hubu and Gongbu application data.
-- Visitor cookies are hashed before persistence; IP addresses and user agents
-- are used only in memory for filtering and are never written here.
CREATE TABLE analytics_visitors (
  visitor_hash TEXT PRIMARY KEY,
  first_seen INTEGER NOT NULL,
  last_seen INTEGER NOT NULL
) STRICT;

CREATE TABLE analytics_events (
  event_id TEXT PRIMARY KEY,
  visitor_hash TEXT NOT NULL REFERENCES analytics_visitors(visitor_hash),
  session_id TEXT NOT NULL,
  recorded_at INTEGER NOT NULL,
  day TEXT NOT NULL,
  path TEXT NOT NULL,
  referrer TEXT NOT NULL,
  country TEXT NOT NULL
) STRICT;

CREATE INDEX analytics_events_recorded_at ON analytics_events(recorded_at);
CREATE INDEX analytics_events_visitor ON analytics_events(visitor_hash);
CREATE INDEX analytics_events_session ON analytics_events(visitor_hash, session_id, recorded_at);
CREATE INDEX analytics_visitors_last_seen ON analytics_visitors(last_seen);

-- Filtered beacons carry no persistent visitor identity or event details.
CREATE TABLE analytics_exclusions (
  day TEXT NOT NULL,
  reason TEXT NOT NULL,
  count INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (day, reason)
) STRICT;
