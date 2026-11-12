-- The first admin account is the super admin: it cannot be removed or changed by anyone else, and only it can remove other admins.
ALTER TABLE users ADD COLUMN `is_super` TINYINT(1) NOT NULL DEFAULT 0;

UPDATE users SET is_super = 1
 WHERE username = (SELECT username FROM (SELECT username FROM users WHERE role = 'admin' AND disabled = 0 ORDER BY created_at, username LIMIT 1) first_admin);
