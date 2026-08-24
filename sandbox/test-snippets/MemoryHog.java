import java.util.ArrayList;
import java.util.List;

public class MemoryHog {
    public static void main(String[] args) {
        List<byte[]> chunks = new ArrayList<>();
        while (true) {
            chunks.add(new byte[10_000_000]); // 10MB por iteração
            System.out.println("alocado: " + chunks.size() * 10 + "MB");
        }
    }
}
