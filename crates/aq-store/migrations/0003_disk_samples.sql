-- 版本 3：磁盘实测按 worktree 区分冷编和增量编，并标记这次实测是否可靠（CASE-EST-16、CASE-EST-17）。
-- worktree：提交时执行目录所在的 worktree 根；不在 git 仓库里为空。
-- disk_reliable：运行期间没有别的执行在跑、aq 也没有在建或删模拟器时为 1；旧记录为空，按不可靠处理。
ALTER TABLE executions ADD COLUMN worktree TEXT;
ALTER TABLE executions ADD COLUMN disk_reliable INTEGER CHECK (disk_reliable IN (0,1));
CREATE INDEX exec_worktree ON executions(profile_key, worktree, finished_at);
