import java.net.InetSocketAddress;
import java.net.Socket;

public class NetworkEscape {
    public static void main(String[] args) {
        try (Socket s = new Socket()) {
            s.connect(new InetSocketAddress("8.8.8.8", 53), 3000);
            System.out.println("FALHA DE ISOLAMENTO: conexão de rede funcionou");
        } catch (Exception e) {
            System.out.println("rede bloqueada como esperado: " + e);
        }
    }
}
