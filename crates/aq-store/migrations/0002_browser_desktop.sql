-- 版本 2：设备类型加 browser、desktop。SQLite 改不了 CHECK，只能重建 hold_devices：
-- 旧表改名 → 按新约束建表 → 拷数据 → 删旧表 → 重建索引。没有别的表引用 hold_devices，改名不牵连外键。
ALTER TABLE hold_devices RENAME TO hold_devices_v1;

CREATE TABLE hold_devices (
  hold_id TEXT NOT NULL REFERENCES holds(id) ON DELETE CASCADE,
  seq INTEGER NOT NULL,
  type TEXT NOT NULL CHECK (type IN ('ios-sim','android-emu','ios-device','android-device','browser','desktop')),
  device TEXT,
  PRIMARY KEY (hold_id, seq)
) STRICT;

INSERT INTO hold_devices (hold_id, seq, type, device) SELECT hold_id, seq, type, device FROM hold_devices_v1;
DROP TABLE hold_devices_v1;
CREATE UNIQUE INDEX one_device_holder ON hold_devices(device);
