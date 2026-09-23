-- The checkout a session works in, when it is not the workspace's own
-- directory: a git worktree of the same repository. Every session in one
-- directory shares its branch, so this is how one session sits on a branch of
-- its own without moving the others. NULL means the workspace root.
--
-- The branch is recorded when the checkout is chosen, for labels; the
-- directory is the authority on what is actually checked out there.
ALTER TABLE sessions ADD COLUMN checkout_path TEXT;
ALTER TABLE sessions ADD COLUMN checkout_branch TEXT;
