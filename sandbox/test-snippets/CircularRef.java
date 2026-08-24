public class CircularRef {
    static class Node {
        int value;
        Node next;
        Node(int value) {
            this.value = value;
        }
    }

    public static void main(String[] args) {
        Node a = new Node(1);
        Node b = new Node(2);
        a.next = b;
        b.next = a; // referência circular

        int[] bigArray = new int[50];
        for (int i = 0; i < bigArray.length; i++) {
            bigArray[i] = i * i;
        }

        System.out.println("a.value=" + a.value + " b.value=" + b.value);
        System.out.println("bigArray criado com " + bigArray.length + " elementos");
    }
}
