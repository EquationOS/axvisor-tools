#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

int main()
{
	const char *device_path = "/dev/axivc_publisher";
	char buffer[128];
	ssize_t bytes_read, bytes_write;

	int fd = open(device_path, O_RDWR);
	if (fd < 0)
	{
		perror("Failed to open device");
		return 1;
	}

	bytes_read = read(fd, buffer, sizeof(buffer) - 1);
	if (bytes_read < 0)
	{
		perror("Failed to read from device");
		close(fd);
		return 1;
	}

	buffer[bytes_read] = '\0';

	printf("Read from device: %s", buffer);

	bytes_write = write(fd, "Hello from user space!\n", 23);
	if (bytes_write < 0)
	{
		perror("Failed to write to device");
		close(fd);
		return 1;
	}
	printf("Wrote %zd bytes to device\n", bytes_write);

	close(fd);
	return 0;
}