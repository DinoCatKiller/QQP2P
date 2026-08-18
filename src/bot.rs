pub struct BotContext {
    pub my_user_id: u64,
    pub my_name: String,
}

impl BotContext {
    pub fn new(my_user_id: u64, my_name: &str) -> Self {
        Self {
            my_user_id,
            my_name: my_name.to_string(),
        }
    }
}
