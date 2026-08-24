import java.nio.file.Files;
import java.nio.file.Paths;

public class FilesystemEscape {
    public static void main(String[] args) throws Exception {
        // tenta ler um arquivo sensível fora do que deveria ser visível ao código do usuário
        try {
            String content = new String(Files.readAllBytes(Paths.get("/etc/shadow")));
            System.out.println("FALHA DE ISOLAMENTO: leu /etc/shadow, " + content.length() + " bytes");
        } catch (Exception e) {
            System.out.println("leitura de /etc/shadow bloqueada como esperado: " + e);
        }

        // tenta escrever fora do diretório de trabalho
        try {
            Files.write(Paths.get("/tmp/escape-test.txt"), "escapei".getBytes());
            System.out.println("escreveu em /tmp (esperado, ainda sem rootfs isolado de verdade)");
        } catch (Exception e) {
            System.out.println("escrita fora do cwd bloqueada: " + e);
        }
    }
}
