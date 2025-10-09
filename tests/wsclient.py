# File: wsclient.py
import asyncio
import websockets
import ssl

# --- Configuration ---
# The URI to connect to. This is Vane's local address.
# We use "wss://" because we are connecting to the TLS/SSL port (443).
VANE_URI = "wss://localhost:443/"

# The custom Host header. Vane will use this for HTTP routing.
CUSTOM_HOST_HEADER = "localhost"

# The message we will send to the echo server.
MESSAGE_TO_SEND = f"Hello Vane, this is a test for {CUSTOM_HOST_HEADER}!"


async def test_vane_websocket_proxy():
    """
    Connects to the Vane proxy with a custom Host header and tests the echo server.
    """
    print(f"Attempting to connect to Vane at: {VANE_URI}")
    print(f"Using custom Host header: {CUSTOM_HOST_HEADER}")

    # --- SSL Context ---
    # Create a custom SSL context to ignore certificate validation.
    # This tells the CLIENT to not reject the server's self-signed certificate.
    ssl_context = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
    ssl_context.check_hostname = False
    ssl_context.verify_mode = ssl.CERT_NONE

    try:
        # The `async with` block ensures the connection is closed properly.
        async with websockets.connect(
            VANE_URI,
            ssl=ssl_context,
            extra_headers={"Host": CUSTOM_HOST_HEADER},
            # FIX: Add `server_hostname` parameter.
            # This tells the TLS layer to send "canmi.icu" as the SNI.
            # Vane needs this to select the correct certificate to serve.
            # This resolves the "TLSV1_ALERT_ACCESS_DENIED" error.
            server_hostname=CUSTOM_HOST_HEADER
        ) as websocket:

            print("\nConnection successful!")

            # Send our test message.
            print(f"> Sending: {MESSAGE_TO_SEND}")
            await websocket.send(MESSAGE_TO_SEND)

            # Wait for the echo response from the server.
            response = await websocket.recv()
            print(f"< Received: {response}")

            # Verify that the echo worked.
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
    # Run the asynchronous test function.
    asyncio.run(test_vane_websocket_proxy())