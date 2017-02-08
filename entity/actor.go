package entity

// Different status types
type Status struct {
	// TO DO
}

// Location
type Cords struct {
	X, Y int
}

// Everything is an entity
type Entity struct {
	Name        string
	Full_hp     int
	C_hp        int
	Phy_def     int
	Phy_atk     int
	Speed       int
	Coordinates Cords
	Destination Cords
}

type EntityObj struct {
	Entities []Entity
}

// Actors will be things that can attack or be attacked...
type Actor interface {
	ReduceHP(int)
	IncreaseHP(int)
	IsDead() bool
	GetStatuses() []Status
	Attack() Actor
	Move(Cords)
	GetCurrentHP() int
	GetMaxHP() int
	GetDefense() int
	GetAttack() int
	GetLocation() Cords
}
