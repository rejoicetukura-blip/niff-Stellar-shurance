use soroban_sdk::{Address, Env};

/// Invoke the SEP-41 `transfer` entry-point on an external token contract.
pub fn transfer(env: &Env, token: &Address, from: &Address, to: &Address, amount: i128) {
    let args = soroban_sdk::vec![
        env,
        soroban_sdk::IntoVal::<Env, soroban_sdk::Val>::into_val(from, env),
        soroban_sdk::IntoVal::<Env, soroban_sdk::Val>::into_val(to, env),
        soroban_sdk::IntoVal::<Env, soroban_sdk::Val>::into_val(&amount, env),
    ];
    env.invoke_contract::<()>(token, &soroban_sdk::Symbol::new(env, "transfer"), args);
}
