pub fn run_migrations() {
    include_str!("../migrations/001_init.sql");
}
