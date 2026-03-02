pub fn run_migrations() {
    include_str!("../migrations/001_init.sql");
    include_str!("../migrations/002_init.sql");
}
