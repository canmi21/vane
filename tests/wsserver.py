# File: wsserver.py
import asyncio
import websockets

async def echo(websocket):
    """
    This function handles a single WebSocket connection.
    It waits for a message and sends the same message back.
    """
    print("Client connected.")
    try:
        async for message in websocket:
            print(f"< Received: {message}")
            await websocket.send(message)
            print(f"> Sent: {message}")
    except websockets.exceptions.ConnectionClosed:
        print("Client disconnected.")

async def main():
    """Starts the WebSocket server."""
    # Ensure your virtual environment is active before running this
    async with websockets.serve(echo, "localhost", 8080):
        print("WebSocket echo server started on ws://localhost:8080")
        await asyncio.Future()  # Run forever

if __name__ == "__main__":
    asyncio.run(main())