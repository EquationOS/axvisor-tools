#include <ivc/ulib.h>

#include <pybind11/pybind11.h>
#include <cstdint>
#include <cstdio>
#include <memory>

namespace py = pybind11;

class ManagerPointerManager {
public:
    ManagerPointerManager() {
        manager = ivc_open_manager();
        if (!manager) {
            py::set_error(PyExc_RuntimeError, "Failed to open IVC manager");
            throw ::std::exception();
        }
    }

    ~ManagerPointerManager() {
        if (manager) {
            if (ivc_close_manager(manager) < 0) {
                py::set_error(PyExc_RuntimeError, "Failed to close IVC manager");
            }
        }
    }

    ivc_manager_p get_manager() const {
        return manager;
    }
private:
    ivc_manager_p manager;
};

typedef ::std::shared_ptr<ManagerPointerManager> ManagerPtr;

class Manager {
public:
    Manager() {
        manager = ::std::make_shared<ManagerPointerManager>();
    }

    ~Manager() {}

    ManagerPtr copy_manager_ptr() const {
        return manager;
    }

private:
    ManagerPtr manager;
};

class Subscriber {
public:
    Subscriber(const Manager& mgr, uint64_t publisher_id, uint64_t channel_key) {
        manager_ptr = mgr.copy_manager_ptr();
        subscriber = ivc_subscribe(manager_ptr->get_manager(), publisher_id, channel_key);
        if (!subscriber) {
            py::set_error(PyExc_RuntimeError, "Failed to subscribe to channel");
            throw ::std::exception();
        }
    }

    ~Subscriber() {
        if (ivc_unsubscribe(subscriber) < 0) {
            py::set_error(PyExc_RuntimeError, "Failed to unsubscribe from channel");
        }
    }

    py::bytes read(size_t max_bytes) {
        if (max_bytes == 0) {
            return py::bytes();
        } else if (max_bytes < 0) {
            py::set_error(PyExc_RuntimeError, "max_bytes must be non-negative");
            throw ::std::exception();
        }

        std::vector<char> buffer(max_bytes);
        int bytes_read = ivc_read(subscriber, buffer.data(), max_bytes);
        if (bytes_read < 0) {
            py::set_error(PyExc_RuntimeError, "Failed to read from subscriber device");
            throw ::std::exception();
        } else if (bytes_read == 0) {
            return py::bytes();
        }

        buffer.resize(bytes_read);
        return py::bytes(buffer.data(), bytes_read);
    }

private:
    ManagerPtr manager_ptr;
    ivc_subscriber_p subscriber;
};

class Publisher {
public:
    Publisher(const Manager& mgr, uint64_t channel_key, uint64_t channel_size) {
        manager_ptr = mgr.copy_manager_ptr();
        publisher = ivc_publish(manager_ptr->get_manager(), channel_key, channel_size);
        if (!publisher) {
            py::set_error(PyExc_RuntimeError, "Failed to publish channel");
            throw ::std::exception();
        }
    }

    ~Publisher() {
        if (ivc_unpublish(publisher) < 0) {
            py::set_error(PyExc_RuntimeError, "Failed to unpublish channel");
        }
    }

    int write(const py::bytes& data) {
        std::string data_str = data;

        if (data_str.size() == 0) {
            return 0; // Nothing to write
        }

        int bytes_written = ivc_write_all(publisher, data_str.data(), data_str.size());
        if (bytes_written < 0) {
            py::set_error(PyExc_RuntimeError, "Failed to write to publisher device");
            throw ::std::exception();
        }
        return bytes_written;
    }

private:
    ManagerPtr manager_ptr;
    ivc_publisher_p publisher;
};

PYBIND11_MODULE(ivcpy, m) {
    m.doc() = "Axvisor IVC Library Python Bindings";

    py::class_<Manager>(m, "Manager")
        .def(py::init<>());

    py::class_<Subscriber>(m, "Subscriber")
        .def(py::init<const Manager&, uint64_t, uint64_t>())
        .def("read", &Subscriber::read);

    py::class_<Publisher>(m, "Publisher")
        .def(py::init<const Manager&, uint64_t, uint64_t>())
        .def("write", &Publisher::write);
}
