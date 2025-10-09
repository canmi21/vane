# File: wsserver.py
import asyncio
import websockets

async def echo(websocket):
    print("Client connected.")
    try:
        async for message in websocket:
            print(f"< Received: {message}")
            await websocket.send(message)
            print(f"> Sent: {message}")
    except websockets.exceptions.ConnectionClosed:
        print("Client disconnected.")

async def main():
    async with websockets.serve(echo, "localhost", 8080):
        print("WebSocket echo server started on ws://localhost:8080")
        await asyncio.Future()

if __name__ == "__main__":
    asyncio.run(main())