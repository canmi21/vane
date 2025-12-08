const std = @import("std");

pub fn main() !void {
    // Allocate I/O buffers
    var stdin_buf: [8192]u8 = undefined;
    var stdout_buf: [1024]u8 = undefined;
    var stderr_buf: [1024]u8 = undefined;

    // Create readers/writers
    var stdin_reader = std.fs.File.stdin().readerStreaming(&stdin_buf);
    var stdout_writer = std.fs.File.stdout().writer(&stdout_buf);
    var stderr_writer = std.fs.File.stderr().writer(&stderr_buf);

    const stderr = &stderr_writer.interface;

    _ = try stderr.writeAll("⚙ Starting execution...\n");
    try stderr.flush();

    // Setup allocator
    var gpa = std.heap.GeneralPurposeAllocator(.{}){};
    defer _ = gpa.deinit();
    const allocator = gpa.allocator();

    // Read all stdin using streamRemaining
    var input_writer: std.Io.Writer.Allocating = .init(allocator);
    defer input_writer.deinit();

    _ = try stdin_reader.interface.streamRemaining(&input_writer.writer);
    const input_raw = try input_writer.toOwnedSlice();
    defer allocator.free(input_raw);

    if (input_raw.len == 0) {
        _ = try stderr.writeAll("✗ No input received on stdin\n");
        try stderr.flush();
        return;
    }

    var buf: [256]u8 = undefined;
    const msg = try std.fmt.bufPrint(&buf, "⚙ Received Input: {s}\n", .{input_raw});
    _ = try stderr.writeAll(msg);
    try stderr.flush();

    // Parse JSON for auth_token
    var auth_token: []const u8 = "";
    const key = "\"auth_token\":\"";
    if (std.mem.indexOf(u8, input_raw, key)) |start| {
        if (std.mem.indexOfPos(u8, input_raw, start + key.len, "\"")) |end| {
            auth_token = input_raw[start + key.len .. end];
        }
    }

    var branch: []const u8 = "";
    var store: []const u8 = "";
    if (std.mem.eql(u8, auth_token, "secret123")) {
        _ = try stderr.writeAll("✓ Auth success\n");
        try stderr.flush();
        branch = "success";
        store = "{\"user_role\":\"admin\",\"verified\":\"true\"}";
    } else {
        const err_msg = try std.fmt.bufPrint(&buf, "✗ Auth failed. Token: {s}\n", .{auth_token});
        _ = try stderr.writeAll(err_msg);
        try stderr.flush();
        branch = "failure";
        store = "{\"error_reason\":\"invalid_token\"}";
    }

    var out_buf: [512]u8 = undefined;
    const output = try std.fmt.bufPrint(&out_buf, "{{\"branch\":\"{s}\",\"store\":{s}}}\n", .{ branch, store });
    _ = try stdout_writer.interface.writeAll(output);
    try stdout_writer.interface.flush();
}
