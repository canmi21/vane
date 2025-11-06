# tests/http/h1/h1_1.py

import socket

def send_request(host: str, port: int):
    """
    Connects to the server, sends a raw HTTP/1.1 GET request,
    and prints the response.
    """
    print("Executing HTTP/1.1 Test")
    print(f"Target: http://{host}:{port}/")

    # Manually construct the raw HTTP/1.1 request.
    request_text = (
        f"GET / HTTP/1.1\r\n"
        f"Host: {host}:{port}\r\n"
        f"User-Agent: Vane HTTP/1.1\r\n"
        f"Connection: close\r\n"
        f"\r\n"
    )

    try:
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
            s.connect((host, port))

            print("\nSending Request")
            print(request_text.strip())

            s.sendall(request_text.encode('utf-8'))

            response_parts = []
            while True:
                data = s.recv(4096)
                if not data:
                    break
                response_parts.append(data)

        full_response = b"".join(response_parts).decode('utf-8', errors='ignore')

        print("\nFull Response Received")
        print(full_response)

    except ConnectionRefusedError:
        print(f"\nConnection refused. Is the Vane engine running on port {port}?")
    except socket.error as e:
        print(f"\nA socket error occurred: {e}")
    except Exception as e:
        print(f"\nAn unexpected error occurred: {e}")