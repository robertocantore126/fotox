"""Fotox — server statico di sviluppo.

Serve la cartella del progetto e aggiunge Cache-Control: no-store, così ogni
ricarica del browser riprende davvero le modifiche (python -m http.server
lascia che il browser usi una cache euristica, che si mangia le modifiche).

Uso:  python tools/serve.py [porta] [cartella] [--no-open]

Note utili quando "non si vede nulla":
  * se la porta richiesta è occupata, ne viene scelta una libera e l'indirizzo
    vero viene stampato qui sotto (prima il server moriva con WinError 10048);
  * ascolta sia su 127.0.0.1 sia su ::1, così http://localhost:PORT/ funziona
    anche dove localhost risolve prima su IPv6;
  * apre il browser sull'indirizzo giusto, a meno di --no-open.
"""

import functools
import http.server
import os
import socket
import socketserver
import sys
import threading
import webbrowser

DEFAULT_PORT = 5500


class Server(socketserver.ThreadingMixIn, http.server.HTTPServer):
    daemon_threads = True
    # Su Windows SO_REUSEADDR consente a *due* processi di ascoltare la stessa
    # porta e le richieste finiscono su un socket a caso: lì va disattivato.
    # Su POSIX serve per riavviare subito dopo un Ctrl+C (socket in TIME_WAIT).
    allow_reuse_address = os.name != "nt"
    address_family = socket.AF_INET  # sostituito per il listener IPv6


class Handler(http.server.SimpleHTTPRequestHandler):
    def end_headers(self):
        self.send_header("Cache-Control", "no-store, must-revalidate")
        self.send_header("Pragma", "no-cache")
        self.send_header("Expires", "0")
        super().end_headers()

    def log_message(self, fmt, *args):  # log sobrio
        sys.stderr.write("  %s\n" % (fmt % args))


def port_in_use(port, host="127.0.0.1"):
    """C'è davvero qualcuno in ascolto? Una connessione di prova lo dice in modo
    affidabile: un bind da solo mente, perché su Windows SO_REUSEADDR fa
    riuscire il secondo bind e i socket in TIME_WAIT fanno fallire quello giusto."""
    try:
        with socket.create_connection((host, port), timeout=0.4):
            return True
    except OSError:
        return False


def free_port(start):
    """La prima porta libera da `start` in su, su 127.0.0.1."""
    for port in range(start, start + 50):
        if not port_in_use(port):
            return port
    raise SystemExit("no free port between %d and %d" % (start, start + 50))


def bind_or_move(port, handler):
    """Ascolta sulla porta richiesta, o sulla prima libera se è occupata."""
    asked = port
    if port_in_use(port):
        print("!! porta %d già occupata da un altro processo (probabilmente un'altra\n   istanza di questo server): ne uso una libera." % port)
        port = free_port(port + 1)
    try:
        return Server(("127.0.0.1", port), handler), port, asked
    except OSError as err:
        print("!! non riesco ad aprire la porta %d (%s)" % (port, getattr(err, "strerror", err)))
        port = free_port(port + 1)
        return Server(("127.0.0.1", port), handler), port, asked


def listen_ipv6(port, handler):
    """Secondo listener su ::1 (best effort: se non riesce, si prosegue)."""

    class V6Server(Server):
        address_family = socket.AF_INET6
        allow_reuse_address = os.name != "nt"

    if port_in_use(port, "::1"):
        return None
    try:
        httpd = V6Server(("::1", port), handler)
    except OSError:
        return None
    threading.Thread(target=httpd.serve_forever, daemon=True).start()
    return httpd


def main():
    # Senza questo, redirigendo l'output (o dentro un altro strumento) il banner
    # con l'indirizzo resta nel buffer e sembra che il server non abbia detto nulla.
    try:
        sys.stdout.reconfigure(line_buffering=True)
    except (AttributeError, ValueError):
        pass

    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    flags = [a for a in sys.argv[1:] if a.startswith("--")]
    port = int(args[0]) if args else DEFAULT_PORT
    root = args[1] if len(args) > 1 else os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    handler = functools.partial(Handler, directory=root)

    if not os.path.isfile(os.path.join(root, "index.html")):
        print("!! %s non contiene index.html: stai servendo la cartella sbagliata." % root)
        print("   Uso atteso:  python tools/serve.py  (dalla cartella fotox)")
        return 1

    try:
        httpd, port, asked = bind_or_move(port, handler)
    except OSError as err:
        print("!! impossibile aprire una porta: %s" % err)
        return 1

    v6 = listen_ipv6(port, handler)

    url = "http://127.0.0.1:%d/" % port
    print("")
    print("  Fotox è servito qui:   %s" % url)
    print("                         http://localhost:%d/" % port)
    print("  radice:                %s" % root)
    if port != asked:
        print("  (porta richiesta: %d)" % asked)
    print("  fermare con Ctrl+C.")
    print("")

    if "--no-open" not in flags:
        threading.Timer(0.4, lambda: webbrowser.open(url)).start()

    try:
        httpd.serve_forever()
    except KeyboardInterrupt:
        print("\nFermato.")
    finally:
        httpd.server_close()
        if v6:
            v6.shutdown()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
