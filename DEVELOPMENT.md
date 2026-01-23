# Development Guide

This guide contains tips, tricks, and best practices for developing Kora.

## Development Workflow

### 1. Branch Strategy
- **Feature branches**: `feature/description` or `fix/description`
- **Main branch**: Always deployable, protected
- **No direct pushes**: Use PRs for all changes

### 2. Commit Messages
Use [Conventional Commits](https://www.conventionalcommits.org/) for automatic versioning:

```bash
# Features (minor version bump)
git commit -m "feat(lib): add Token2022 support"
git commit -m "feat(rpc): implement new signAndSend method"

# Bug fixes (patch version bump)  
git commit -m "fix(cli): handle invalid keypair format"
git commit -m "fix(rpc): validate transaction signatures"

# Breaking changes (major version bump)
git commit -m "feat(lib)!: change signer interface"
git commit -m "feat: remove deprecated methods

BREAKING CHANGE: removed getBalance method, use getAccountBalance instead"

# Other types (patch version bump)
git commit -m "chore(deps): update solana-sdk to 2.1.10"
git commit -m "docs(readme): add installation instructions"
git commit -m "refactor(lib): simplify token validation logic"
```

### 3. Pull Request Process
1. **Create feature branch**: `git checkout -b feat/my-feature`
2. **Make changes** with conventional commits
3. **Add tests** for new functionality
4. **Update docs** if needed
5. **Create PR** with descriptive title and body
6. **Address review feedback**
7. **Merge** (squash merge preferred)

## Rent Reclaim Bot
The rent-reclaim-bot is a CLI tool that helps Kora operators reclaim rent from sponsored accounts.

### Building the bot
To build the bot, run the following command:
```bash
cargo build --release -p rent-reclaim-bot
```

### Running the bot
To run the bot, you will need to create a `config.json` file with the following content:
```json
{
  "rpc_url": "https://api.devnet.solana.com",
  "database_path": "rent-reclaim.db",
  "kora_signer": "YOUR_KORA_SIGNER_PUBLIC_KEY",
  "treasury_wallet": "YOUR_TREASURY_WALLET_PUBLIC_KEY",
  "operator_keypair": "YOUR_OPERATOR_KEYPAIR_BASE58_STRING"
}
```

You can then run the bot using the following command:
```bash
./target/release/rent-reclaim-bot --config config.json <COMMAND>
```

The following commands are available:
- `scan`: Scan the blockchain for new sponsored accounts
- `monitor`: Monitor the status of the sponsored accounts
- `report`: Generate a report of the sponsored accounts
- `reclaim`: Reclaim rent from a specific account
- `close-account`: Close a sponsored account and reclaim the rent