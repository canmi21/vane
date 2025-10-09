# File: wsclient.py
import asyncio
import websockets
import ssl

# --- Configuration ---
# FIX: Connect to a specific path, not the root.
VANE_URI = "wss://localhost:443/ws/echo"

CUSTOM_HOST_HEADER = "localhost"
MESSAGE_TO_SEND = f"Hello Vane, this is a test for {CUSTOM_HOST_HEADER}!"

async def test_vane_websocket_proxy():
    print(f"Attempting to connect to Vane at: {VANE_URI}")
    print(f"Using custom Host header: {CUSTOM_HOST_HEADER}")

    ssl_context = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
    ssl_context.check_hostname = False
    ssl_context.verify_mode = ssl.CERT_NONE

    try:
        async with websockets.connect(
            VANE_URI,
            ssl=ssl_context,
            extra_headers={"Host": CUSTOM_HOST_HEADER},
            server_hostname=CUSTOM_HOST_HEADER
        ) as websocket:

            print("\nConnection successful!")
            print(f"> Sending: {MESSAGE_TO_SEND}")
            await websocket.send(MESSAGE_TO_SEND)
            response = await websocket.recv()
            print(f"< Received: {response}")

            if response == MESSAGE_TO_SEND:
                print("\nSUCCESS: The received message matches the sent message.")
            else:
                print("\nFAILURE: The messages do not match.")

    except websockets.exceptions.ConnectionClosedError as e:
        print(f"\nERROR: Connection closed unexpectedly. Code: {e.code}, Reason: {e.reason}")
        print("    -> Check if the backend WebSocket server is running.")
        print("    -> Check Vane's logs for errors.")
    except ConnectionRefusedError:
        print("\nERROR: Connection refused.")
        print("    -> Is Vane running and listening on port 443?")
    except Exception as e:
        print(f"\nAn unexpected error occurred: {e}")

if __name__ == "__main__":
    asyncio.run(test_vane_websocket_proxy())