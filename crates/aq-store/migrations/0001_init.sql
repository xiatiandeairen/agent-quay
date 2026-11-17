-- 版本 1：文档「数据存储结构」schema v2。只做加法：新结构写新的迁移文件，不改本文件（红区）。
CREATE TABLE executions (
  id TEXT PRIMARY KEY,
  hash TEXT,
  status TEXT NOT NULL CHECK (status IN ('queued','running','ok','failed','timeout','interrupted','cancelled','rejected')),
  cwd TEXT NOT NULL,
  argv TEXT NOT NULL CHECK (json_valid(argv)),
  profile_key TEXT NOT NULL,
  fingerprint TEXT,
  content_changed INTEGER CHECK (content_changed IN (0,1)),
  heavy INTEGER CHECK (heavy IN (0,1)),
  mem_mb INTEGER,
  disk_mb INTEGER,
  uses_net INTEGER CHECK (uses_net IN (0,1)),
  timeout_s INTEGER,
  submitted_at INTEGER NOT NULL,
  started_at INTEGER,
  finished_at INTEGER,
  pgid INTEGER,
  pgid_started_at INTEGER,
  cancel_requested INTEGER NOT NULL DEFAULT 0 CHECK (cancel_requested IN (0,1)),
  exit_code INTEGER,
  peak_mem_mb INTEGER,
  cpu_avg_cores REAL,
  disk_delta_mb INTEGER,
  CHECK (status <> 'running' OR (started_at IS NOT NULL AND pgid IS NOT NULL AND pgid_started_at IS NOT NULL AND heavy IS NOT NULL)),
  CHECK (status IN ('queued','running') OR finished_at IS NOT NULL),
  CHECK ((exit_code IS NOT NULL) = (status IN ('ok','failed'))),
  CHECK (status <> 'ok' OR exit_code = 0),
  CHECK (status <> 'failed' OR exit_code <> 0)
) STRICT;

CREATE TABLE profiles (
  profile_key TEXT PRIMARY KEY,
  signature TEXT NOT NULL,
  heavy INTEGER NOT NULL CHECK (heavy IN (0,1)),
  mem_mb INTEGER NOT NULL,
  disk_mb INTEGER NOT NULL,
  uses_net INTEGER NOT NULL CHECK (uses_net IN (0,1)),
  est_s INTEGER NOT NULL,
  ai_output TEXT NOT NULL
) STRICT;

CREATE TABLE holds (
  id TEXT PRIMARY KEY,
  status TEXT NOT NULL CHECK (status IN ('waiting','held','released','reclaimed','rejected')),
  request TEXT NOT NULL CHECK (json_valid(request)),
  owner_pid INTEGER NOT NULL,
  owner_started_at INTEGER NOT NULL,
  requested_at INTEGER NOT NULL,
  acquired_at INTEGER,
  ended_at INTEGER,
  CHECK (status <> 'held' OR acquired_at IS NOT NULL),
  CHECK (status IN ('waiting','held') OR ended_at IS NOT NULL)
) STRICT;

CREATE TABLE hold_devices (
  hold_id TEXT NOT NULL REFERENCES holds(id) ON DELETE CASCADE,
  seq INTEGER NOT NULL,
  type TEXT NOT NULL CHECK (type IN ('ios-sim','android-emu','ios-device','android-device')),
  device TEXT,
  PRIMARY KEY (hold_id, seq)
) STRICT;

CREATE TABLE hold_accounts (
  account TEXT PRIMARY KEY,
  hold_id TEXT NOT NULL REFERENCES holds(id) ON DELETE CASCADE
) STRICT;

CREATE UNIQUE INDEX one_inflight ON executions(hash) WHERE status IN ('queued','running');
CREATE UNIQUE INDEX one_device_holder ON hold_devices(device);
CREATE INDEX exec_queue ON executions(status, submitted_at);
CREATE INDEX exec_profile ON executions(profile_key, finished_at);
CREATE INDEX hold_queue ON holds(status, requested_at);

CREATE TRIGGER executions_terminal_immutable BEFORE UPDATE OF status ON executions
WHEN OLD.status IN ('ok','failed','timeout','interrupted','cancelled','rejected') AND NEW.status <> OLD.status
BEGIN SELECT RAISE(ABORT, 'terminal status is immutable'); END;

CREATE TRIGGER holds_terminal_immutable BEFORE UPDATE OF status ON holds
WHEN OLD.status IN ('released','reclaimed','rejected') AND NEW.status <> OLD.status
BEGIN SELECT RAISE(ABORT, 'terminal status is immutable'); END;
