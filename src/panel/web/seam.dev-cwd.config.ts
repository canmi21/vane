export default {
  backend: {
    port: 3334,
    devCommand: {
      command: "cargo watch -x 'locate-project --workspace --message-format plain'",
      cwd: "../../.."
    }
  }
}
